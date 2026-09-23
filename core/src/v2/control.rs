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
        "create_instance" => create_instance(tx, session_id, params, identity),
        "spawn_instance" => spawn_instance(tx, session_id, params, identity),
        "issue_grant" => issue_grant(tx, session_id, params, identity),
        "revoke_grant" => revoke_grant(tx, session_id, params, identity),
        "reauthorize_operation" => reauthorize_operation(tx, session_id, params),
        "reset_instance" => reset_instance(tx, session_id, params, identity),
        "create_goal" => create_goal(tx, session_id, params, identity),
        "send_message" => send_message(tx, session_id, params, identity),
        "drain_inbox" => drain_inbox(tx, session_id, params, identity),
        "delegate_task" => delegate_task(tx, session_id, params, identity),
        "start_task" => start_task(tx, session_id, params, identity),
        "complete_task" => complete_task(tx, session_id, params, identity),
        "cancel_task" => cancel_task(tx, session_id, params, identity),
        "read_history" => read_history(tx, session_id, params, identity),
        "fire_timer" => fire_timer(tx, session_id, params, identity),
        "blocked_report" => blocked_report(tx, session_id, params, identity),
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
        "register_check_runs" => register_check_runs(tx, session_id, params, identity),
        "repair_completion" => repair_completion(tx, session_id, params, identity),
        "block_goal" => block_goal(tx, session_id, params, identity),
        "close_completion" => close_completion(tx, session_id, params),
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

/// Session grant revision (§5.1): every issue/revoke bumps it; dispatch
/// compares it against the operation's stamped revision (§6.1).
fn grant_revision(tx: &Connection) -> Result<i64, String> {
    let value: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key = 'grant_revision'", [], |row| row.get(0))
        .optional()
        .map_err(|e| format!("grant revision: {e}"))?;
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(0))
}

fn bump_grant_revision(tx: &Connection) -> Result<i64, String> {
    let next = grant_revision(tx)? + 1;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('grant_revision', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [next.to_string()],
    )
    .map_err(|e| format!("bump grant revision: {e}"))?;
    Ok(next)
}

/// Scope coverage (§5.1): "session" covers the whole session; otherwise the
/// granted scope must equal or be a path prefix of the requested one.
fn scope_covers(granted: &str, requested: &str) -> bool {
    granted == "session" || granted == requested || requested.starts_with(&format!("{granted}/"))
}

/// The most specific active grant covering (subject, action, resource).
fn active_grant(
    tx: &Connection,
    subject: &str,
    action: &str,
    resource: &str,
) -> Result<Option<(String, String)>, String> {
    let mut stmt = tx
        .prepare("SELECT id, resource_scope FROM grants WHERE subject = ?1 AND action = ?2 AND revoked_at IS NULL")
        .map_err(|e| format!("grant read: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![subject, action], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("grant query: {e}"))?;
    let mut best: Option<(String, String)> = None;
    for row in rows {
        let (id, scope) = row.map_err(|e| format!("grant row: {e}"))?;
        if scope_covers(&scope, resource) && best.as_ref().is_none_or(|(_, held)| scope.len() > held.len()) {
            best = Some((id, scope));
        }
    }
    Ok(best)
}

fn authorized(tx: &Connection, subject: &str, action: &str, resource: &str) -> Result<bool, String> {
    Ok(active_grant(tx, subject, action, resource)?.is_some())
}

/// The capability a tool intent needs (§5.1): shell touches the explicitly
/// granted shared workspace; collaboration intents need their connection or
/// management grant — re-checked at the dispatch linearization point (§6.1,
/// A04), so a revocation between import and dispatch fails the operation.
fn capability_gap(tx: &Connection, instance: &str, intent: &Json) -> Result<Option<String>, String> {
    let gap = match intent["name"].as_str() {
        Some("shell") if !authorized(tx, instance, "shell", "workspace")? => {
            Some(format!("instance {instance} holds no shell@workspace grant"))
        }
        Some(crate::kernel::SEND_TOOL) => {
            let recipient = intent["args"]["recipient"].as_str().unwrap_or("");
            let scope = format!("instance:{recipient}");
            if authorized(tx, instance, "message", &scope)? {
                None
            } else {
                Some(format!("instance {instance} holds no message grant over {recipient}"))
            }
        }
        Some(crate::kernel::DELEGATE_TOOL) => {
            let assignee = intent["args"]["assignee"].as_str().unwrap_or("");
            let scope = format!("instance:{assignee}");
            if authorized(tx, instance, "delegate", &scope)? {
                None
            } else {
                Some(format!("instance {instance} holds no delegate grant over {assignee}"))
            }
        }
        Some(crate::kernel::SPAWN_TOOL) if !authorized(tx, instance, "manage", "session")? => {
            Some(format!("instance {instance} holds no manage grant over the session"))
        }
        _ => None,
    };
    Ok(gap)
}

/// Issue a scoped grant (§5.1): the User is the root of authority; an
/// instance may only narrow what it holds, and a manage-grant holder may in
/// addition issue message/delegate grants inside its scope (management of
/// connections, Q5). Every issue bumps the session grant revision.
fn issue_grant(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let subject = params["subject"].as_str().ok_or("issue_grant.subject required")?;
    let action = params["action"].as_str().ok_or("issue_grant.action required")?;
    let scope = params["resource_scope"].as_str().ok_or("issue_grant.resource_scope required")?;
    if subject.is_empty() || action.is_empty() || scope.is_empty() {
        return Err("issue_grant: subject/action/resource_scope must not be empty".into());
    }
    let parent: Option<String> = match identity {
        Identity::User => params["parent_grant_id"].as_str().map(str::to_string),
        Identity::System => return Err("the system identity cannot issue grants".into()),
        Identity::Instance(issuer) => {
            // manage covers issuing connection grants; everything else is
            // strict narrowing of the issuer's own same-action grant
            let covering = if matches!(action, "message" | "delegate") {
                active_grant(tx, issuer, "manage", scope)?.or(active_grant(tx, issuer, action, scope)?)
            } else {
                active_grant(tx, issuer, action, scope)?
            };
            let Some((parent_id, _)) = covering else {
                return Err(format!("instance {issuer} holds no grant covering {action}@{scope}"));
            };
            Some(parent_id)
        }
    };
    if let Some(parent_id) = &parent {
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT action, resource_scope FROM grants WHERE id = ?1 AND revoked_at IS NULL",
                [parent_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("parent grant: {e}"))?;
        let Some((p_action, p_scope)) = row else {
            return Err(format!("parent grant {parent_id} is missing or revoked"));
        };
        let action_ok = p_action == action || (p_action == "manage" && matches!(action, "message" | "delegate"));
        if !action_ok || !scope_covers(&p_scope, scope) {
            return Err(format!("parent grant {parent_id} does not cover {action}@{scope}"));
        }
    }
    let id = format!("g-{}", uuid::Uuid::new_v4());
    tx.execute(
        "INSERT INTO grants (id, session_id, issuer, subject, action, resource_scope, parent_grant_id, revision, revoked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, NULL)",
        rusqlite::params![id, session_id, identity.actor(), subject, action, scope, parent],
    )
    .map_err(|e| format!("grant insert: {e}"))?;
    let revision = bump_grant_revision(tx)?;
    event(
        tx,
        session_id,
        "grant_issued",
        subject,
        &json!({"grant_id": id, "subject": subject, "action": action, "resource_scope": scope,
                "parent_grant_id": parent, "revision": revision}),
    )?;
    Ok(json!({"grant_id": id, "revision": revision}))
}

/// Revoke a grant (§5.1): the User may revoke any grant; an instance may
/// revoke only grants it issued. Derived grants (transitively parented) are
/// revoked in the same transaction. Revocation blocks subsequent dispatch;
/// in-flight authorized operations are cancelled on request, not rewritten
/// (§5.4/§6.1).
fn revoke_grant(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let id = params["grant_id"].as_str().ok_or("revoke_grant.grant_id required")?;
    let issuer: String = tx
        .query_row(
            "SELECT issuer FROM grants WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![id, session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("grant {id}: {e}"))?;
    match identity {
        Identity::User => {}
        Identity::Instance(me) if *me == issuer => {}
        _ => return Err("only the user or the issuing instance may revoke a grant".into()),
    }
    let revoked = revoke_grant_tree(tx, id)?;
    let revision = bump_grant_revision(tx)?;
    event(tx, session_id, "grant_revoked", id, &json!({"grant_id": id, "cascade": revoked, "revision": revision}))?;
    Ok(json!({"grant_id": id, "revoked": revoked, "revision": revision}))
}

/// Revoke a grant and every active grant derived from it, transitively
/// (§5.1). Returns the ids actually revoked; the caller bumps the session
/// grant revision once for the whole batch.
fn revoke_grant_tree(tx: &Connection, root: &str) -> Result<Vec<String>, String> {
    let mut ids = vec![root.to_string()];
    let mut frontier = vec![root.to_string()];
    while let Some(next) = frontier.pop() {
        let mut stmt = tx
            .prepare("SELECT id FROM grants WHERE parent_grant_id = ?1 AND revoked_at IS NULL")
            .map_err(|e| format!("grant children: {e}"))?;
        let children: Vec<String> = stmt
            .query_map([&next], |row| row.get::<_, String>(0))
            .map_err(|e| format!("grant children query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("grant children collect: {e}"))?;
        frontier.extend(children.iter().cloned());
        ids.extend(children);
    }
    let now = crate::models::now();
    let mut revoked = Vec::new();
    for id in ids {
        let changed = tx
            .execute(
                "UPDATE grants SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                rusqlite::params![now, id],
            )
            .map_err(|e| format!("revoke {id}: {e}"))?;
        if changed == 1 {
            revoked.push(id);
        }
    }
    Ok(revoked)
}

/// Close every open execution artifact of one instance epoch in the same
/// transaction as the lifecycle change (§5.4): pending requests close and
/// release their budget reservations, queued operations cancel before any
/// side effect, in-flight operations keep only the persisted cancel request
/// for the driver, and pending waits plus unapplied envelopes of the epoch
/// are sealed so nothing stale reaches the next epoch.
fn close_epoch_execution(
    tx: &Connection,
    session_id: &str,
    instance_id: &str,
    epoch: i64,
    reason: &str,
) -> Result<Json, String> {
    let pending_requests: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT request_id FROM model_requests WHERE instance_id = ?1 AND epoch = ?2 AND status = 'PENDING'",
            )
            .map_err(|e| format!("close requests: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![instance_id, epoch], |row| row.get(0))
            .map_err(|e| format!("close requests query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("close requests collect: {e}"))?
    };
    for request in &pending_requests {
        tx.execute("UPDATE model_requests SET status = 'CANCELLED' WHERE request_id = ?1", [request])
            .map_err(|e| format!("close request {request}: {e}"))?;
        release_reservation(tx, request)?;
    }
    let operations: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT o.operation_id, o.status FROM operations o
                 JOIN decisions d ON o.decision_id = d.decision_id
                 JOIN model_requests r ON d.request_id = r.request_id
                 WHERE r.instance_id = ?1 AND r.epoch = ?2
                   AND o.status IN ('PREPARED', 'DISPATCH_COMMITTED', 'RUNNING')",
            )
            .map_err(|e| format!("close operations: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![instance_id, epoch], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("close operations query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("close operations collect: {e}"))?
    };
    let mut cancelled = 0i64;
    let mut flagged = 0i64;
    for (operation_id, status) in &operations {
        if status == "PREPARED" {
            let receipt = json!({"operation_id": operation_id, "ok": false, "started": false,
                                 "content": json!({"error": reason}).to_string(),
                                 "error": {"class": "cancelled", "reason": reason}});
            tx.execute(
                "UPDATE operations SET status = 'CANCELLED', receipt_json = ?2 WHERE operation_id = ?1",
                rusqlite::params![operation_id, receipt.to_string()],
            )
            .map_err(|e| format!("close operation {operation_id}: {e}"))?;
            expire_pending_approvals(tx, operation_id)?;
            cancelled += 1;
        } else {
            tx.execute("UPDATE operations SET cancel_requested = 1 WHERE operation_id = ?1", [operation_id])
                .map_err(|e| format!("flag operation {operation_id}: {e}"))?;
            flagged += 1;
        }
    }
    let waits = tx
        .execute(
            "UPDATE waits SET status = 'CANCELLED' WHERE instance_id = ?1 AND epoch = ?2 AND status = 'PENDING'",
            rusqlite::params![instance_id, epoch],
        )
        .map_err(|e| format!("close waits: {e}"))?;
    let envelopes = tx
        .execute(
            "UPDATE envelopes SET state = 'SUPERSEDED'
             WHERE recipient = ?1 AND epoch = ?2 AND session_id = ?3 AND state = 'ACCEPTED'",
            rusqlite::params![instance_id, epoch, session_id],
        )
        .map_err(|e| format!("seal envelopes: {e}"))?;
    Ok(json!({"requests_closed": pending_requests.len(), "operations_cancelled": cancelled,
              "operations_cancel_requested": flagged, "waits_closed": waits, "envelopes_sealed": envelopes}))
}

/// Reset (§5.4, Q9): the instance keeps its id, goal and tasks but starts a
/// fresh context epoch. Everything from the old epoch closes in the same
/// transaction; late receipts of the old epoch keep landing on their
/// original operations and budget without entering the new epoch (A24).
fn reset_instance(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let instance_id = params["instance_id"].as_str().ok_or("reset_instance.instance_id required")?;
    match identity {
        Identity::User => {}
        Identity::System => return Err("the system cannot reset instances".into()),
        Identity::Instance(actor) => {
            let scope = format!("instance:{instance_id}");
            if !authorized(tx, actor, "manage", &scope)? {
                return Err(format!("instance {actor} holds no manage grant over {instance_id}"));
            }
        }
    }
    let reason = params["reason"].as_str().unwrap_or("reset by user");
    let (session, epoch, lifecycle, _, _): (String, i64, String, String, i64) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if lifecycle == "TERMINATED" {
        return Err(format!("instance {instance_id} is terminated"));
    }
    let closed = close_epoch_execution(tx, session_id, instance_id, epoch, reason)?;
    tx.execute(
        "UPDATE instances SET context_epoch = context_epoch + 1, phase = 'READY', active_request_id = NULL,
         revision = revision + 1 WHERE id = ?1",
        [instance_id],
    )
    .map_err(|e| format!("reset instance: {e}"))?;
    event(
        tx,
        session_id,
        "instance_reset",
        instance_id,
        &json!({"old_epoch": epoch, "new_epoch": epoch + 1, "reason": reason, "closed": closed}),
    )?;
    Ok(json!({"instance_id": instance_id, "epoch": epoch + 1, "closed": closed}))
}

/// Re-authorization after a grant change (§5.4/§6.1): a PREPARED operation
/// must prove its capability again before dispatch. Still-authorized ops
/// are re-stamped to the current revision; unauthorized ones are cancelled
/// before any side effect and the decision is woken for consumption.
fn reauthorize_operation(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let operation_id = params["operation_id"].as_str().ok_or("reauthorize_operation.operation_id required")?;
    let (decision_id, status, intent_json): (String, String, String) = tx
        .query_row(
            "SELECT decision_id, status, intent_json FROM operations WHERE operation_id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("operation {operation_id}: {e}"))?;
    if status != "PREPARED" {
        return Err(format!("operation {operation_id} is {status}, not reauthorizable"));
    }
    let instance: String = tx
        .query_row(
            "SELECT r.instance_id FROM decisions d JOIN model_requests r ON d.request_id = r.request_id
             WHERE d.decision_id = ?1",
            [&decision_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("decision instance: {e}"))?;
    let intent: Json = serde_json::from_str(&intent_json).map_err(|e| format!("intent {operation_id}: {e}"))?;
    let current = grant_revision(tx)?;
    if let Some(reason) = capability_gap(tx, &instance, &intent)? {
        let receipt = json!({"operation_id": operation_id, "ok": false, "started": false,
                             "content": json!({"error": reason}).to_string(),
                             "error": {"class": "unauthorized", "reason": reason}});
        tx.execute(
            "UPDATE operations SET status = 'CANCELLED', receipt_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![operation_id, receipt.to_string()],
        )
        .map_err(|e| format!("unauthorized cancel: {e}"))?;
        expire_pending_approvals(tx, operation_id)?;
        let open = consume_if_closed(tx, session_id, &decision_id)?;
        event(tx, session_id, "operation_unauthorized", operation_id, &json!({"reason": reason}))?;
        return Ok(json!({"operation_id": operation_id, "status": "CANCELLED", "decision_open": open}));
    }
    tx.execute(
        "UPDATE operations SET grant_revision = ?1 WHERE operation_id = ?2",
        rusqlite::params![current, operation_id],
    )
    .map_err(|e| format!("re-stamp {operation_id}: {e}"))?;
    Ok(json!({"operation_id": operation_id, "status": "PREPARED", "grant_revision": current}))
}

fn create_instance(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
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
    if !workspace.is_empty() && matches!(identity, Identity::User) {
        // the default shared project directory is an explicitly granted
        // resource (§5.1); creation grants no other connection or file
        // scope. Spawned instances get theirs through issue_grant (spawn).
        let grant_id = format!("g-{}", uuid::Uuid::new_v4());
        tx.execute(
            "INSERT INTO grants (id, session_id, issuer, subject, action, resource_scope, parent_grant_id, revision, revoked_at)
             VALUES (?1, ?2, 'user', ?3, 'shell', 'workspace', NULL, 0, NULL)",
            rusqlite::params![grant_id, session_id, id],
        )
        .map_err(|e| format!("workspace grant: {e}"))?;
        bump_grant_revision(tx)?;
    }
    event(tx, session_id, "instance_created", id, &json!({"instance_id": id}))?;
    Ok(json!({"instance_id": id, "phase": "READY", "revision": 0, "epoch": 0}))
}

/// Atomic spawn (§5.2): create the instance, derive the spawner's delegate
/// authority over it from its manage grant, and register the optional
/// initial task with its narrow return path — one transaction, so a spawned
/// worker never exists without its task and return channel. Creation itself
/// grants no connections or file scopes (§5.1).
fn spawn_instance(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let spawner = match identity {
        Identity::User => "user".to_string(),
        Identity::System => return Err("the system does not spawn instances".into()),
        Identity::Instance(actor) => {
            if !authorized(tx, actor, "manage", "session")? {
                return Err(format!("instance {actor} holds no manage grant over the session"));
            }
            actor.clone()
        }
    };
    let instance_id = params["instance_id"].as_str().ok_or("spawn_instance.instance_id required")?;
    if let Some(instructions) = params["instructions"].as_str() {
        if instructions.is_empty() {
            return Err("spawn_instance.instructions must not be empty".into());
        }
    }
    let create = json!({"id": instance_id, "profile": params.get("profile").cloned().unwrap_or(json!({})),
                        "workspace_ref": params["workspace_ref"].as_str().unwrap_or("")});
    // Identity::Instance here on purpose: spawn grants no automatic
    // shell@workspace — that path belongs to user-driven creation only (§5.1)
    create_instance(tx, session_id, &create, &Identity::Instance(spawner.clone()))?;
    if spawner != "user" {
        // the spawner's delegate authority over the new instance is derived
        // from its manage grant (management of connections, Q5)
        issue_grant(
            tx,
            session_id,
            &json!({"subject": spawner, "action": "delegate", "resource_scope": format!("instance:{instance_id}")}),
            &Identity::Instance(spawner.clone()),
        )?;
    }
    let mut task = Json::Null;
    if let Some(description) = params["task"].as_str() {
        let task_id =
            params["task_id"].as_str().map(str::to_string).unwrap_or_else(|| format!("t-{}", uuid::Uuid::new_v4()));
        let mut delegate = json!({"task_id": task_id, "assignee": instance_id, "description": description});
        if let Some(goal) = params["goal_id"].as_str() {
            delegate["goal_id"] = json!(goal);
        }
        if let Some(acceptance) = params.get("acceptance_refs") {
            delegate["acceptance_refs"] = acceptance.clone();
        }
        let delegation_identity = if spawner == "user" { Identity::User } else { Identity::Instance(spawner.clone()) };
        task = delegate_task(tx, session_id, &delegate, &delegation_identity)?;
    }
    event(tx, session_id, "instance_spawned", instance_id, &json!({"spawner": spawner, "task": !task.is_null()}))?;
    Ok(json!({"instance_id": instance_id, "task": task}))
}

/// Default inbox capacity per recipient (§5.3): a full inbox fails the
/// send openly so the sender can retry later — nothing is silently dropped.
const DEFAULT_MAX_INBOX: i64 = 64;

/// One queued envelope before insertion (§5.3).
struct Outbox<'a> {
    sender: &'a str,
    recipient: &'a str,
    epoch: i64,
    kind: &'a str,
    correlation_id: Option<&'a str>,
    payload: &'a Json,
}

/// Insert a queued envelope for a recipient of this session. Shared by
/// messages, task assignment, results and cancellations (§5.3): the
/// envelope persists inside the caller's transaction and the recipient
/// applies it once at a safe boundary; receiving alone never wakes a turn.
fn queue_envelope(tx: &Connection, session_id: &str, outbox: &Outbox) -> Result<String, String> {
    let envelope_id = format!("m-{}", uuid::Uuid::new_v4());
    let sequence: i64 = tx
        .query_row("SELECT COALESCE(MAX(sequence), 0) + 1 FROM envelopes WHERE session_id = ?1", [session_id], |row| {
            row.get(0)
        })
        .map_err(|e| format!("envelope sequence: {e}"))?;
    tx.execute(
        "INSERT INTO envelopes
         (id, session_id, sender, recipient, epoch, kind, correlation_id, payload_json, sequence, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'ACCEPTED')",
        rusqlite::params![
            envelope_id,
            session_id,
            outbox.sender,
            outbox.recipient,
            outbox.epoch,
            outbox.kind,
            outbox.correlation_id,
            outbox.payload.to_string(),
            sequence
        ],
    )
    .map_err(|e| format!("envelope {envelope_id}: {e}"))?;
    Ok(envelope_id)
}

/// Instance-to-instance message (§5.3, Q5): the sender must hold a message
/// grant covering the recipient; the envelope queues for the recipient's
/// boundary drain. A full inbox is an explicit backpressure error.
fn send_message(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let recipient = params["recipient"].as_str().ok_or("send_message.recipient required")?;
    let text = params["text"].as_str().ok_or("send_message.text required")?;
    let (session, epoch, lifecycle, _, _) = load_instance(tx, recipient)?;
    if session != session_id {
        return Err(format!("instance {recipient} does not belong to this session"));
    }
    if lifecycle == "TERMINATED" {
        return Err(format!("recipient {recipient} is terminated"));
    }
    let sender = match identity {
        Identity::User => "user".to_string(),
        Identity::System => return Err("the system does not send messages".into()),
        Identity::Instance(actor) => {
            let scope = format!("instance:{recipient}");
            if !authorized(tx, actor, "message", &scope)? {
                return Err(format!("instance {actor} holds no message grant over {recipient}"));
            }
            actor.clone()
        }
    };
    let max_inbox = params["max_inbox"].as_i64().unwrap_or(DEFAULT_MAX_INBOX);
    let queued: i64 = tx
        .query_row("SELECT COUNT(*) FROM envelopes WHERE recipient = ?1 AND state = 'ACCEPTED'", [recipient], |row| {
            row.get(0)
        })
        .map_err(|e| format!("inbox read: {e}"))?;
    if queued >= max_inbox {
        return Err(format!("recipient {recipient} inbox full ({queued}/{max_inbox}): backpressure"));
    }
    let correlation = params["correlation_id"].as_str();
    let envelope_id = queue_envelope(
        tx,
        session_id,
        &Outbox {
            sender: &sender,
            recipient,
            epoch,
            kind: "message",
            correlation_id: correlation,
            payload: &json!({"text": text}),
        },
    )?;
    event(
        tx,
        session_id,
        "message_sent",
        recipient,
        &json!({"envelope_id": envelope_id, "sender": sender, "correlation_id": correlation}),
    )?;
    Ok(json!({"envelope_id": envelope_id, "queued": true}))
}

/// The context text one queued envelope becomes (§5.3): the structured
/// sender/kind stay on the envelope row; the model sees a plain user-role
/// line with the origin prefixed.
fn envelope_text(kind: &str, sender: &str, correlation: Option<&str>, payload: &Json) -> String {
    let text =
        payload["text"].as_str().or(payload["description"].as_str()).or(payload["summary"].as_str()).unwrap_or("");
    match (kind, correlation) {
        ("message", _) => format!("[message from {sender}] {text}"),
        ("task_assigned", Some(task)) => format!("[task {task} assigned by {sender}] {text}"),
        ("task_result", Some(task)) => format!("[task {task} result from {sender}] {text}"),
        ("task_cancelled", Some(task)) => format!("[task {task} cancelled by {sender}] {text}"),
        (other, _) => format!("[{other} from {sender}] {text}"),
    }
}

/// Drain the recipient inbox at a safe boundary (§5.3, A06): every queued
/// envelope of the current epoch applies once in sequence order — the
/// apply-dedup makes a replayed drain a no-op — and stale-epoch leftovers
/// seal as SUPERSEDED instead of leaking into the new epoch.
fn drain_inbox(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let instance_id = params["instance_id"].as_str().ok_or("drain_inbox.instance_id required")?;
    match identity {
        Identity::User => {}
        Identity::Instance(actor) if *actor == instance_id => {}
        _ => return Err("only the user or the instance itself may drain its inbox".into()),
    }
    let (session, epoch, lifecycle, _, _) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if lifecycle == "TERMINATED" {
        return Err(format!("instance {instance_id} is terminated"));
    }
    let queued: Vec<(String, String, i64, String, Option<String>, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, sender, epoch, kind, correlation_id, payload_json FROM envelopes
                 WHERE recipient = ?1 AND session_id = ?2 AND state = 'ACCEPTED' ORDER BY sequence",
            )
            .map_err(|e| format!("inbox read: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![instance_id, session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })
            .map_err(|e| format!("inbox query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("inbox collect: {e}"))?
    };
    let mut applied = 0i64;
    let mut sealed = 0i64;
    for (id, sender, envelope_epoch, kind, correlation, payload_json) in queued {
        if envelope_epoch != epoch {
            tx.execute("UPDATE envelopes SET state = 'SUPERSEDED' WHERE id = ?1", [&id])
                .map_err(|e| format!("seal envelope {id}: {e}"))?;
            sealed += 1;
            continue;
        }
        let payload: Json = serde_json::from_str(&payload_json).unwrap_or(json!({}));
        let content = envelope_text(&kind, &sender, correlation.as_deref(), &payload);
        append_context(tx, instance_id, epoch, "user", &json!({"role": "user", "content": content}), Some(&id), &[])?;
        tx.execute("UPDATE envelopes SET state = 'APPLIED' WHERE id = ?1", [&id])
            .map_err(|e| format!("apply envelope {id}: {e}"))?;
        applied += 1;
    }
    if applied > 0 {
        // the context moved: any executor holding a pre-drain snapshot is
        // stale now (§6.1 single-executor revision check)
        tx.execute("UPDATE instances SET revision = revision + 1 WHERE id = ?1", [instance_id])
            .map_err(|e| format!("instance revision: {e}"))?;
    }
    if applied > 0 || sealed > 0 {
        event(tx, session_id, "inbox_drained", instance_id, &json!({"applied": applied, "sealed": sealed}))?;
    }
    // applied envelopes are wait facts: satisfying a pending wait closes it
    // and wakes the instance in this same transaction (§5.3, A23)
    let woken = wake_satisfied(tx, session_id)?;
    let revision: i64 = tx
        .query_row("SELECT revision FROM instances WHERE id = ?1", [instance_id], |row| row.get(0))
        .map_err(|e| format!("instance revision read: {e}"))?;
    Ok(json!({"instance_id": instance_id, "applied": applied, "sealed": sealed, "woken": woken,
              "revision": revision}))
}

/// Delegate a task (§5.2/§5.3): the requester atomically registers the task,
/// the narrow return capability (task_result@task:<id>, just enough to
/// settle this task — no general reverse channel) and the assignment
/// envelope. Dependencies must already exist; a fresh task id cannot close
/// a dependency cycle, so explicit prerequisites stay acyclic (§5.3).
fn delegate_task(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let task_id = params["task_id"].as_str().ok_or("delegate_task.task_id required")?;
    let assignee = params["assignee"].as_str().ok_or("delegate_task.assignee required")?;
    let description = params["description"].as_str().unwrap_or("");
    let acceptance = params.get("acceptance_refs").cloned().unwrap_or(json!([]));
    let dependencies = params.get("dependencies").cloned().unwrap_or(json!([]));
    let (session, epoch, lifecycle, _, _) = load_instance(tx, assignee)?;
    if session != session_id {
        return Err(format!("instance {assignee} does not belong to this session"));
    }
    if lifecycle == "TERMINATED" {
        return Err(format!("assignee {assignee} is terminated"));
    }
    let (requester, parent_grant) = match identity {
        Identity::User => ("user".to_string(), None),
        Identity::System => return Err("the system does not delegate tasks".into()),
        Identity::Instance(actor) => {
            let scope = format!("instance:{assignee}");
            let Some((grant_id, _)) = active_grant(tx, actor, "delegate", &scope)? else {
                return Err(format!("instance {actor} holds no delegate grant over {assignee}"));
            };
            (actor.clone(), Some(grant_id))
        }
    };
    let goal_id = match params["goal_id"].as_str() {
        Some(goal) => goal.to_string(),
        None => {
            if requester == "user" {
                return Err("delegate_task.goal_id required when the user delegates".into());
            }
            tx.query_row("SELECT active_goal_id FROM instances WHERE id = ?1", [&requester], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(|e| format!("requester goal: {e}"))?
            .ok_or("delegate_task.goal_id required: requester has no active goal")?
        }
    };
    let deps: Vec<String> = dependencies
        .as_array()
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    for dep in &deps {
        if dep == task_id {
            return Err("task must not depend on itself".into());
        }
        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM tasks WHERE id = ?1 AND session_id = ?2",
                rusqlite::params![dep, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("dependency {dep}: {e}"))?;
        if exists.is_none() {
            return Err(format!("dependency {dep} does not exist in this session"));
        }
    }
    tx.execute(
        "INSERT INTO tasks (id, goal_id, session_id, requester, assignee, dependencies_json,
                            acceptance_refs_json, status, result_refs_json, revision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'PENDING', '[]', 0)",
        rusqlite::params![
            task_id,
            goal_id,
            session_id,
            requester,
            assignee,
            dependencies.to_string(),
            acceptance.to_string()
        ],
    )
    .map_err(|e| format!("delegate_task {task_id}: {e}"))?;
    let return_grant = format!("g-{}", uuid::Uuid::new_v4());
    tx.execute(
        "INSERT INTO grants (id, session_id, issuer, subject, action, resource_scope, parent_grant_id, revision, revoked_at)
         VALUES (?1, ?2, ?3, ?4, 'task_result', ?5, ?6, 0, NULL)",
        rusqlite::params![return_grant, session_id, requester, assignee, format!("task:{task_id}"), parent_grant],
    )
    .map_err(|e| format!("return grant: {e}"))?;
    bump_grant_revision(tx)?;
    let envelope_id = queue_envelope(
        tx,
        session_id,
        &Outbox {
            sender: &requester,
            recipient: assignee,
            epoch,
            kind: "task_assigned",
            correlation_id: Some(task_id),
            payload: &json!({"task_id": task_id, "description": description, "acceptance_refs": acceptance}),
        },
    )?;
    event(
        tx,
        session_id,
        "task_delegated",
        assignee,
        &json!({"task_id": task_id, "requester": requester, "goal_id": goal_id, "envelope_id": envelope_id}),
    )?;
    Ok(json!({"task_id": task_id, "status": "PENDING", "return_grant": return_grant, "envelope_id": envelope_id}))
}

fn load_task(tx: &Connection, session_id: &str, task_id: &str) -> Result<(String, String, String, String), String> {
    tx.query_row(
        "SELECT goal_id, requester, assignee, status FROM tasks WHERE id = ?1 AND session_id = ?2",
        rusqlite::params![task_id, session_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .map_err(|e| format!("task {task_id}: {e}"))
}

/// The assignee starts its task (PENDING → RUNNING).
fn start_task(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let task_id = params["task_id"].as_str().ok_or("start_task.task_id required")?;
    let (_, _, assignee, status) = load_task(tx, session_id, task_id)?;
    match identity {
        Identity::User => {}
        Identity::Instance(actor) if *actor == assignee => {}
        _ => return Err("only the user or the assignee may start a task".into()),
    }
    if status != "PENDING" {
        return Err(format!("task {task_id} is {status}, not startable"));
    }
    tx.execute("UPDATE tasks SET status = 'RUNNING', revision = revision + 1 WHERE id = ?1", [task_id])
        .map_err(|e| format!("start task {task_id}: {e}"))?;
    event(tx, session_id, "task_started", &assignee, &json!({"task_id": task_id}))?;
    Ok(json!({"task_id": task_id, "status": "RUNNING"}))
}

/// Settle or report a task (§5.3): the assignee proves the narrow return
/// capability for exactly this task; the result is delivered only to a
/// still-available requester, and a terminal settlement consumes the return
/// grant in the same transaction.
fn complete_task(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let task_id = params["task_id"].as_str().ok_or("complete_task.task_id required")?;
    let target = params["status"].as_str().ok_or("complete_task.status required")?;
    if !matches!(target, "SUCCEEDED" | "FAILED" | "BLOCKED") {
        return Err(format!("complete_task status {target:?} must be SUCCEEDED, FAILED or BLOCKED"));
    }
    let summary = params["summary"].as_str().unwrap_or("");
    let result_refs = params.get("result_refs").cloned().unwrap_or(json!([]));
    let (_, requester, assignee, status) = load_task(tx, session_id, task_id)?;
    // terminal tasks answer from the stored state before any authorization
    // work — the return grant may already be consumed by the first settle
    if matches!(status.as_str(), "SUCCEEDED" | "FAILED" | "CANCELLED") {
        if status == target {
            return Ok(json!({"task_id": task_id, "status": status, "replayed": true}));
        }
        return Err(format!("task {task_id} is already {status}"));
    }
    match identity {
        Identity::User => {}
        Identity::Instance(actor) => {
            if *actor != assignee {
                return Err(format!("instance {actor} is not the assignee of task {task_id}"));
            }
            let scope = format!("task:{task_id}");
            if !authorized(tx, &assignee, "task_result", &scope)? {
                return Err(format!("instance {assignee} holds no task_result grant for {task_id}"));
            }
        }
        Identity::System => return Err("the system does not complete tasks".into()),
    }
    tx.execute(
        "UPDATE tasks SET status = ?1, result_refs_json = ?2, revision = revision + 1 WHERE id = ?3",
        rusqlite::params![target, result_refs.to_string(), task_id],
    )
    .map_err(|e| format!("complete task {task_id}: {e}"))?;
    // a settlement reported from the assignee's finish turn closes that
    // turn (mirror of complete_goal's phase rule); conditional, so settling
    // mid-turn or by the user never rewires a live phase
    let closed_turn = tx
        .execute(
            "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'COMPLETION_PENDING'",
            [&assignee],
        )
        .map_err(|e| format!("settlement phase: {e}"))?;
    if closed_turn == 1 {
        // queue continuation (§5.3): with further open tasks the instance
        // must not park on its own assistant tail — the settlement note is
        // the input that lets the loop advance to the next task
        let remaining: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE assignee = ?1 AND status IN ('PENDING', 'RUNNING')",
                [&assignee],
                |row| row.get(0),
            )
            .map_err(|e| format!("queue depth: {e}"))?;
        if remaining > 0 {
            let epoch: i64 = tx
                .query_row("SELECT context_epoch FROM instances WHERE id = ?1", [&assignee], |row| row.get(0))
                .map_err(|e| format!("assignee epoch: {e}"))?;
            append_context(
                tx,
                &assignee,
                epoch,
                "note",
                &json!({"role": "user", "content": format!(
                    "[task {task_id} settled: {target}] {remaining} open task(s) remain in your queue"
                )}),
                Some(&format!("settle-note-{task_id}")),
                &[],
            )?;
        }
    }
    let terminal = matches!(target, "SUCCEEDED" | "FAILED");
    let mut delivered = false;
    if requester != "user" {
        let alive: Option<String> = tx
            .query_row("SELECT lifecycle FROM instances WHERE id = ?1", [&requester], |row| row.get(0))
            .optional()
            .map_err(|e| format!("requester {requester}: {e}"))?;
        if alive.as_deref().is_some_and(|state| state != "TERMINATED") {
            let epoch: i64 = tx
                .query_row("SELECT context_epoch FROM instances WHERE id = ?1", [&requester], |row| row.get(0))
                .map_err(|e| format!("requester epoch: {e}"))?;
            queue_envelope(
                tx,
                session_id,
                &Outbox {
                    sender: &assignee,
                    recipient: &requester,
                    epoch,
                    kind: "task_result",
                    correlation_id: Some(task_id),
                    payload: &json!({"task_id": task_id, "status": target, "summary": summary, "result_refs": result_refs}),
                },
            )?;
            delivered = true;
        }
    }
    if terminal {
        // a settled task consumes its return path (§5.3)
        let grants: Vec<String> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM grants WHERE subject = ?1 AND action = 'task_result'
                     AND resource_scope = ?2 AND revoked_at IS NULL",
                )
                .map_err(|e| format!("return grants: {e}"))?;
            let rows = stmt
                .query_map(rusqlite::params![assignee, format!("task:{task_id}")], |row| row.get(0))
                .map_err(|e| format!("return grants query: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("return grants collect: {e}"))?
        };
        let mut revoked = false;
        for id in grants {
            revoked |= !revoke_grant_tree(tx, &id)?.is_empty();
        }
        if revoked {
            bump_grant_revision(tx)?;
        }
    }
    event(
        tx,
        session_id,
        "task_completed",
        &requester,
        &json!({"task_id": task_id, "status": target, "assignee": assignee, "delivered": delivered}),
    )?;
    let woken = wake_satisfied(tx, session_id)?;
    Ok(json!({"task_id": task_id, "status": target, "delivered": delivered, "woken": woken}))
}

/// Cancel a task (§5.3): the requester or the user closes it, the return
/// path dies with it and the assignee learns via a queued envelope.
fn cancel_task(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let task_id = params["task_id"].as_str().ok_or("cancel_task.task_id required")?;
    let reason = params["reason"].as_str().unwrap_or("cancelled by requester");
    let (_, requester, assignee, status) = load_task(tx, session_id, task_id)?;
    match identity {
        Identity::User => {}
        Identity::Instance(actor) if *actor == requester => {}
        _ => return Err("only the user or the requester may cancel a task".into()),
    }
    if matches!(status.as_str(), "SUCCEEDED" | "FAILED" | "CANCELLED") {
        return Ok(json!({"task_id": task_id, "status": status, "already_terminal": true}));
    }
    tx.execute("UPDATE tasks SET status = 'CANCELLED', revision = revision + 1 WHERE id = ?1", [task_id])
        .map_err(|e| format!("cancel task {task_id}: {e}"))?;
    let grants: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT id FROM grants WHERE subject = ?1 AND action = 'task_result'
                 AND resource_scope = ?2 AND revoked_at IS NULL",
            )
            .map_err(|e| format!("return grants: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![assignee, format!("task:{task_id}")], |row| row.get(0))
            .map_err(|e| format!("return grants query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("return grants collect: {e}"))?
    };
    let mut revoked = false;
    for id in grants {
        revoked |= !revoke_grant_tree(tx, &id)?.is_empty();
    }
    if revoked {
        bump_grant_revision(tx)?;
    }
    let alive: Option<String> = tx
        .query_row("SELECT lifecycle FROM instances WHERE id = ?1", [&assignee], |row| row.get(0))
        .optional()
        .map_err(|e| format!("assignee {assignee}: {e}"))?;
    if alive.as_deref().is_some_and(|state| state != "TERMINATED") {
        let epoch: i64 = tx
            .query_row("SELECT context_epoch FROM instances WHERE id = ?1", [&assignee], |row| row.get(0))
            .map_err(|e| format!("assignee epoch: {e}"))?;
        queue_envelope(
            tx,
            session_id,
            &Outbox {
                sender: &requester,
                recipient: &assignee,
                epoch,
                kind: "task_cancelled",
                correlation_id: Some(task_id),
                payload: &json!({"task_id": task_id, "reason": reason}),
            },
        )?;
    }
    event(tx, session_id, "task_cancelled", &assignee, &json!({"task_id": task_id, "reason": reason}))?;
    let woken = wake_satisfied(tx, session_id)?;
    Ok(json!({"task_id": task_id, "status": "CANCELLED", "woken": woken}))
}

/// Controlled history entry (A05, Q8): the user reads every instance; an
/// instance reads only its own context — anything else fails closed.
fn read_history(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let instance_id = params["instance_id"].as_str().ok_or("read_history.instance_id required")?;
    match identity {
        Identity::User => {}
        Identity::Instance(actor) if *actor == instance_id => {}
        Identity::Instance(actor) => {
            return Err(format!("instance {actor} may not read the private history of {instance_id}"));
        }
        Identity::System => return Err("the system does not read history".into()),
    }
    let (session, epoch, _, _, _) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    let epoch = params["epoch"].as_i64().unwrap_or(epoch);
    let limit = params["limit"].as_i64().unwrap_or(200);
    let entries: Vec<Json> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, idx, kind, message_json, refs_json FROM context_entries
                 WHERE instance_id = ?1 AND epoch = ?2 ORDER BY idx LIMIT ?3",
            )
            .map_err(|e| format!("history read: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![instance_id, epoch, limit], |row| {
                Ok(json!({"id": row.get::<_, String>(0)?, "idx": row.get::<_, i64>(1)?,
                          "kind": row.get::<_, String>(2)?,
                          "message": serde_json::from_str::<Json>(&row.get::<_, String>(3)?).unwrap_or(Json::Null),
                          "refs": serde_json::from_str::<Json>(&row.get::<_, String>(4)?).unwrap_or(json!([]))}))
            })
            .map_err(|e| format!("history query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("history collect: {e}"))?
    };
    Ok(json!({"instance_id": instance_id, "epoch": epoch, "entries": entries}))
}

fn create_goal(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let id = params["id"].as_str().ok_or("create_goal.id required")?;
    let original = params["original_request_ref"].as_str().unwrap_or("");
    let limits = params.get("limits").cloned().unwrap_or(json!({}));
    let deadline = params["deadline"].as_f64();
    let attach = params["instance_id"].as_str();
    if let Some(checks) = limits.get("required_checks") {
        // user/project predefined machine contracts only (§8): conditions a
        // model distills from natural language carry provenance and can never
        // masquerade as user-confirmed required checks
        if !matches!(identity, Identity::User | Identity::System) {
            return Err(
                "create_goal limits.required_checks: only the user or the project bootstrap predefines required checks"
                    .into(),
            );
        }
        validate_required_checks(checks)?;
    }
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
        // a fresh input makes the instance runnable again at the next
        // boundary; pending waits are superseded — the model sees the input
        // and re-registers what it still needs (§5.3/§5.4)
        tx.execute(
            "UPDATE waits SET status = 'CANCELLED' WHERE instance_id = ?1 AND status = 'PENDING'",
            [instance_id],
        )
        .map_err(|e| format!("supersede waits: {e}"))?;
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
    // A18: a goal-less instance (a worker) shares the budget of the goal its
    // task queue serves — running work first, then the pending queue, FIFO —
    // so reservations and usage from every instance stay visible on the goal
    let goal_id = match goal_id {
        Some(goal) => Some(goal),
        None => tx
            .query_row(
                "SELECT goal_id FROM tasks WHERE assignee = ?1 AND status IN ('RUNNING', 'PENDING')
                 ORDER BY CASE status WHEN 'RUNNING' THEN 0 ELSE 1 END, rowid LIMIT 1",
                [instance_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("task goal read: {e}"))?,
    };
    if let Some(goal) = goal_id.as_deref() {
        // A35: past the goal deadline no new request begins — the daemon
        // honors the deadline even if an eval client expired or was killed.
        // A committed refusal with an auditable event, same as the budget
        // gate; the request never registers and the instance stays READY.
        if goal_deadline_passed(tx, goal)? {
            event(
                tx,
                session_id,
                "goal_deadline_refused",
                instance_id,
                &json!({"goal_id": goal, "request_id": request_id}),
            )?;
            return Ok(json!({"request_id": request_id, "deadline_refused": true,
                             "reason": format!("goal {goal} deadline passed")}));
        }
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

/// Goal deadline gate (A35): the absolute deadline lives on the goal; once
/// it passes, new requests and new side-effect dispatches are refused.
fn goal_deadline_passed(tx: &Connection, goal_id: &str) -> Result<bool, String> {
    let deadline: Option<f64> = tx
        .query_row("SELECT deadline FROM goals WHERE id = ?1", [goal_id], |row| row.get(0))
        .map_err(|e| format!("goal deadline read: {e}"))?;
    Ok(deadline.is_some_and(|d| crate::models::now() > d))
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
/// Where one wait condition stands against the persisted facts (§5.3):
/// already true, still possible, or impossible. Wake uses SATISFIED; the
/// blocked diagnostic uses DEAD so an ordinary wait cycle among live
/// instances is never misreported as a deadlock (A22).
enum ConditionState {
    Satisfied,
    Pending,
    Dead,
}

fn condition_state(
    tx: &Connection,
    session_id: &str,
    instance_id: &str,
    epoch: i64,
    condition: &Json,
) -> Result<ConditionState, String> {
    match condition["kind"].as_str() {
        Some("message") => {
            let (applied, sender_alive): (i64, bool) = match condition["from"].as_str() {
                Some(from) => {
                    let applied: i64 = tx
                        .query_row(
                            "SELECT COUNT(*) FROM envelopes WHERE recipient = ?1 AND sender = ?2
                             AND kind = 'message' AND state = 'APPLIED' AND epoch = ?3",
                            rusqlite::params![instance_id, from, epoch],
                            |row| row.get(0),
                        )
                        .map_err(|e| format!("wait message: {e}"))?;
                    let alive = if from == "user" {
                        true
                    } else {
                        tx.query_row("SELECT lifecycle FROM instances WHERE id = ?1", [from], |row| {
                            row.get::<_, String>(0)
                        })
                        .optional()
                        .map_err(|e| format!("wait sender: {e}"))?
                        .is_some_and(|state| state != "TERMINATED")
                    };
                    (applied, alive)
                }
                None => {
                    let applied: i64 = tx
                        .query_row(
                            "SELECT COUNT(*) FROM envelopes WHERE recipient = ?1 AND kind = 'message'
                             AND state = 'APPLIED' AND epoch = ?2",
                            rusqlite::params![instance_id, epoch],
                            |row| row.get(0),
                        )
                        .map_err(|e| format!("wait message: {e}"))?;
                    (applied, true)
                }
            };
            if applied > 0 {
                return Ok(ConditionState::Satisfied);
            }
            if sender_alive {
                Ok(ConditionState::Pending)
            } else {
                Ok(ConditionState::Dead)
            }
        }
        Some("envelope") => {
            let id = condition["envelope_id"].as_str().ok_or("wait envelope condition needs envelope_id")?;
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM envelopes WHERE id = ?1 AND recipient = ?2",
                    rusqlite::params![id, instance_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("wait envelope: {e}"))?;
            Ok(match state.as_deref() {
                Some("APPLIED") => ConditionState::Satisfied,
                Some("ACCEPTED") => ConditionState::Pending,
                _ => ConditionState::Dead,
            })
        }
        Some("task") => {
            let task_id = condition["task_id"].as_str().ok_or("wait task condition needs task_id")?;
            let status: Option<String> = tx
                .query_row(
                    "SELECT status FROM tasks WHERE id = ?1 AND session_id = ?2",
                    rusqlite::params![task_id, session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("wait task: {e}"))?;
            Ok(match status.as_deref() {
                Some("SUCCEEDED" | "FAILED" | "CANCELLED") => ConditionState::Satisfied,
                Some(_) => ConditionState::Pending,
                None => ConditionState::Dead,
            })
        }
        Some("operation") => {
            let operation_id =
                condition["operation_id"].as_str().ok_or("wait operation condition needs operation_id")?;
            let status: Option<String> = tx
                .query_row("SELECT status FROM operations WHERE operation_id = ?1", [operation_id], |row| row.get(0))
                .optional()
                .map_err(|e| format!("wait operation: {e}"))?;
            Ok(match status.as_deref() {
                Some(status) if is_terminal_op(status) => ConditionState::Satisfied,
                Some(_) => ConditionState::Pending,
                None => ConditionState::Dead,
            })
        }
        other => Err(format!("unknown wait condition kind {other:?}")),
    }
}

/// One registered wait's own definition (§5.3): the branch mode, the
/// conditions and the optional deadline.
struct WaitSpec<'a> {
    mode: &'a str,
    conditions: &'a [Json],
    timer_at: Option<f64>,
}

/// Evaluate a wait against current facts (§5.3): satisfied when the mode
/// holds or the timer is already due; blocked when no branch can ever fire
/// and no live timer remains — reported, never auto-cancelled (A22).
fn evaluate_wait(
    tx: &Connection,
    session_id: &str,
    instance_id: &str,
    epoch: i64,
    spec: &WaitSpec,
    now: f64,
) -> Result<(bool, bool), String> {
    let (mode, conditions, timer_at) = (spec.mode, spec.conditions, spec.timer_at);
    let timer_due = timer_at.is_some_and(|at| at <= now);
    let timer_open = timer_at.is_some_and(|at| at > now);
    let mut any = false;
    let mut all = true;
    let mut dead = 0usize;
    for condition in conditions {
        match condition_state(tx, session_id, instance_id, epoch, condition)? {
            ConditionState::Satisfied => any = true,
            ConditionState::Pending => all = false,
            ConditionState::Dead => {
                all = false;
                dead += 1;
            }
        }
    }
    let satisfied = timer_due || (mode == "ALL" && all) || (mode == "ANY" && any);
    let blocked = !timer_open && !satisfied && (mode == "ALL" && dead > 0 || mode == "ANY" && dead == conditions.len());
    Ok((satisfied, blocked))
}

/// Close every pending wait whose conditions now hold, waking its instance
/// in the same transaction as the fact that satisfied it (§5.3, A23).
/// Returns the satisfied wait ids.
fn wake_satisfied(tx: &Connection, session_id: &str) -> Result<Vec<String>, String> {
    wake_satisfied_at(tx, session_id, crate::models::now())
}

fn wake_satisfied_at(tx: &Connection, session_id: &str, now: f64) -> Result<Vec<String>, String> {
    let pending: Vec<(String, String, i64, String, String, Option<f64>)> = {
        let mut stmt = tx
            .prepare(
                "SELECT w.id, w.instance_id, w.epoch, w.mode, w.conditions_json, w.timer_at FROM waits w
                 JOIN instances i ON w.instance_id = i.id WHERE w.status = 'PENDING' AND i.session_id = ?1",
            )
            .map_err(|e| format!("wake scan: {e}"))?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })
            .map_err(|e| format!("wake query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("wake collect: {e}"))?
    };
    let mut woken = Vec::new();
    for (wait_id, instance_id, epoch, mode, conditions_json, timer_at) in pending {
        let conditions: Vec<Json> = serde_json::from_str(&conditions_json).unwrap_or_default();
        let spec = WaitSpec { mode: &mode, conditions: &conditions, timer_at };
        let (satisfied, _) = evaluate_wait(tx, session_id, &instance_id, epoch, &spec, now)?;
        if !satisfied {
            continue;
        }
        tx.execute("UPDATE waits SET status = 'SATISFIED' WHERE id = ?1", [&wait_id])
            .map_err(|e| format!("wake {wait_id}: {e}"))?;
        // the wake reason joins the context: without it the last entry would
        // be the instance's own assistant turn and the loop would park idle
        // instead of letting the model continue (§5.3). Dedup key = wait id;
        // the PENDING → SATISFIED guard above fires once.
        append_context(
            tx,
            &instance_id,
            epoch,
            "note",
            &json!({"role": "user", "content": format!("[wait {wait_id} satisfied] mode={mode} conditions={conditions_json} timer_at={timer_at:?}")}),
            Some(&wait_id),
            &[],
        )?;
        // WAITING → READY is the ready-intent registration of §3; a phase
        // that already advanced (user input, reset) is not rewritten
        tx.execute(
            "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'WAITING'",
            [&instance_id],
        )
        .map_err(|e| format!("wake phase: {e}"))?;
        event(tx, session_id, "wait_satisfied", &instance_id, &json!({"wait_id": wait_id}))?;
        woken.push(wait_id);
    }
    Ok(woken)
}

/// Timer pass (§5.3): waits whose timer fell due close satisfied and wake
/// their instances. `now` may be passed by the caller so one clock reading
/// covers the whole sweep.
fn fire_timer(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::User | Identity::System) {
        return Err("only the user or the system may fire timers".into());
    }
    let now = params["now"].as_f64().unwrap_or_else(crate::models::now);
    Ok(json!({"satisfied": wake_satisfied_at(tx, session_id, now)?}))
}

/// Blocked-wait diagnostic (A22): pending waits that can never fire again —
/// every reachable branch dead and no live timer — are reported with their
/// reasons. Ordinary wait cycles among live instances and waits with open
/// external paths (running jobs, pending tasks, future timers) are not
/// reported, and nothing is cancelled automatically.
fn blocked_report(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::User | Identity::System) {
        return Err("only the user or the system may request a blocked report".into());
    }
    let _ = params;
    let pending: Vec<(String, String, i64, String, String, Option<f64>)> = {
        let mut stmt = tx
            .prepare(
                "SELECT w.id, w.instance_id, w.epoch, w.mode, w.conditions_json, w.timer_at FROM waits w
                 JOIN instances i ON w.instance_id = i.id WHERE w.status = 'PENDING' AND i.session_id = ?1",
            )
            .map_err(|e| format!("blocked scan: {e}"))?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })
            .map_err(|e| format!("blocked query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("blocked collect: {e}"))?
    };
    let mut blocked = Vec::new();
    let mut waiting = 0usize;
    for (wait_id, instance_id, epoch, mode, conditions_json, timer_at) in pending {
        let conditions: Vec<Json> = serde_json::from_str(&conditions_json).unwrap_or_default();
        let spec = WaitSpec { mode: &mode, conditions: &conditions, timer_at };
        let (_, is_blocked) = evaluate_wait(tx, session_id, &instance_id, epoch, &spec, crate::models::now())?;
        if is_blocked {
            blocked.push(json!({"wait_id": wait_id, "instance_id": instance_id, "mode": mode,
                                "conditions": conditions, "timer_at": timer_at}));
        } else {
            waiting += 1;
        }
    }
    Ok(json!({"blocked": blocked, "waiting": waiting}))
}

fn import_response(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let request_id = params["request_id"].as_str().ok_or("import_response.request_id required")?;
    let decision_id = params["decision_id"].as_str().ok_or("import_response.decision_id required")?;
    let entry_message = params.get("entry").ok_or("import_response.entry required")?;
    let intents = params["intents"].as_array().cloned().unwrap_or_default();
    let completion = params.get("completion").cloned();
    let wait = params.get("wait").cloned();
    // one response registers tools, a wait or a completion — never several
    // at once (§3); a plain reply carries none of them
    if wait.is_some() && (!intents.is_empty() || completion.is_some()) {
        return Err("import_response: a wait excludes tool intents and completion".into());
    }
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
    let grant_revision = match params["grant_revision"].as_i64() {
        Some(revision) => revision,
        None => grant_revision(tx)?,
    };
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
    // a wait registers and is checked against the facts that already hold
    // in the same transaction (§5.3): a result that arrived first still
    // satisfies it, so no wake is ever lost (A23)
    let mut wait_registered: Option<(String, bool)> = None;
    if let Some(wait) = &wait {
        let mode = wait["mode"].as_str().unwrap_or("");
        if !matches!(mode, "ALL" | "ANY") {
            return Err("import_response.wait.mode must be ALL or ANY".into());
        }
        let conditions: Vec<Json> = wait["conditions"].as_array().cloned().unwrap_or_default();
        if conditions.is_empty() {
            return Err("import_response.wait.conditions must not be empty".into());
        }
        // timer_at is absolute; timer_seconds is relative to this
        // transaction's single clock reading (§5.3)
        let timer_at = match (wait["timer_at"].as_f64(), wait["timer_seconds"].as_f64()) {
            (Some(at), _) => Some(at),
            (None, Some(seconds)) if seconds > 0.0 => Some(crate::models::now() + seconds),
            (None, Some(_)) => return Err("import_response.wait.timer_seconds must be positive".into()),
            (None, None) => None,
        };
        let spec = WaitSpec { mode, conditions: &conditions, timer_at };
        let (satisfied, _) = evaluate_wait(tx, session_id, &request_instance, epoch, &spec, crate::models::now())?;
        let wait_id = format!("w-{decision_id}");
        tx.execute(
            "INSERT INTO waits (id, instance_id, epoch, mode, conditions_json, timer_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                wait_id,
                request_instance,
                epoch,
                mode,
                json!(conditions).to_string(),
                timer_at,
                if satisfied { "SATISFIED" } else { "PENDING" }
            ],
        )
        .map_err(|e| format!("wait {wait_id}: {e}"))?;
        wait_registered = Some((wait_id, satisfied));
    }
    let phase = if completion.is_some() {
        "COMPLETION_PENDING"
    } else if !intents.is_empty() {
        "TOOLS_PENDING"
    } else if wait_registered.as_ref().is_some_and(|(_, satisfied)| !satisfied) {
        "WAITING"
    } else {
        "READY"
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
    Ok(json!({"decision_id": decision_id, "phase": phase, "operations": intents.len(),
              "wait": wait_registered.map(|(id, satisfied)| json!({"wait_id": id, "satisfied": satisfied}))}))
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
    // §6.3/A09: an operation that crossed the execution boundary without a
    // verifiable outcome parks the instance's running tasks and notifies;
    // independent work continues. Never silently completable.
    if status == "OUTCOME_UNKNOWN" {
        park_tasks_for_unknown(tx, session_id, &decision_id, operation_id)?;
    }
    expire_pending_approvals(tx, operation_id)?;
    publish_list(tx, session_id, params)?;
    let open = consume_if_closed(tx, session_id, &decision_id)?;
    event(tx, session_id, "operation_completed", operation_id, &json!({"status": status}))?;
    let woken = wake_satisfied(tx, session_id)?;
    Ok(json!({"operation_id": operation_id, "status": status, "decision_open": open, "woken": woken}))
}

/// Park the running tasks of the operation's instance (A09): the honest
/// BLOCKED state plus a task_blocked event per task is the notification;
/// the receipt in context tells the instance not to blindly redo.
fn park_tasks_for_unknown(
    tx: &Connection,
    session_id: &str,
    decision_id: &str,
    operation_id: &str,
) -> Result<(), String> {
    let instance: String = tx
        .query_row(
            "SELECT r.instance_id FROM decisions d JOIN model_requests r ON d.request_id = r.request_id
             WHERE d.decision_id = ?1",
            [decision_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("decision instance: {e}"))?;
    let tasks: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT id FROM tasks WHERE assignee = ?1 AND status = 'RUNNING'")
            .map_err(|e| format!("running tasks: {e}"))?;
        let rows =
            stmt.query_map([&instance], |row| row.get::<_, String>(0)).map_err(|e| format!("running tasks: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("running tasks: {e}"))?
    };
    for task in tasks {
        tx.execute(
            "UPDATE tasks SET status = 'BLOCKED', revision = revision + 1 WHERE id = ?1 AND status = 'RUNNING'",
            [&task],
        )
        .map_err(|e| format!("park task {task}: {e}"))?;
        event(
            tx,
            session_id,
            "task_blocked",
            &task,
            &json!({"task_id": task, "reason": "outcome_unknown", "operation_id": operation_id}),
        )?;
    }
    Ok(())
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
    let permission_revision = match params["permission_revision"].as_i64() {
        Some(revision) => revision,
        None => grant_revision(tx)?,
    };
    let (decision_id, status, args_hash, grant_revision, intent_json): (String, String, String, i64, String) = tx
        .query_row(
            "SELECT decision_id, status, args_hash, grant_revision, intent_json FROM operations WHERE operation_id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
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
    let (instance, op_goal): (String, Option<String>) = tx
        .query_row(
            "SELECT r.instance_id, r.goal_id FROM decisions d JOIN model_requests r ON d.request_id = r.request_id
             WHERE d.decision_id = ?1",
            [&decision_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("decision instance: {e}"))?;
    let (_, _, lifecycle, _, _) = load_instance(tx, &instance)?;
    // A35: no new side effects past the goal deadline either — the driver
    // lands this as a dispatch_refused receipt and the turn closes out.
    if let Some(goal) = op_goal.as_deref() {
        if goal_deadline_passed(tx, goal)? {
            return Err(format!("operation {operation_id} refused: goal {goal} deadline passed"));
        }
    }
    if lifecycle != "ACTIVE" {
        return Err(format!("instance {instance} is {lifecycle}, not dispatching"));
    }
    // authorization is re-checked at dispatch, the linearization point (§6.1)
    let intent: Json = serde_json::from_str(&intent_json).map_err(|e| format!("intent {operation_id}: {e}"))?;
    if let Some(reason) = capability_gap(tx, &instance, &intent)? {
        return Err(format!("operation {operation_id} unauthorized: {reason}"));
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
    // exhaustion, permanent errors) — never resume, terminate or dispatch;
    // an instance needs a manage grant over the target (§5.1, Q5)
    let instance_id = params["instance_id"].as_str().ok_or("set_lifecycle.instance_id required")?;
    match identity {
        Identity::User => {}
        Identity::System => {
            if params["lifecycle"].as_str() != Some("PARKED") {
                return Err("the system may only park instances".into());
            }
        }
        Identity::Instance(actor) => {
            if params["lifecycle"].as_str() == Some("TERMINATED") {
                return Err("termination stays with the user".into());
            }
            let scope = format!("instance:{instance_id}");
            if !authorized(tx, actor, "manage", &scope)? {
                return Err(format!("instance {actor} holds no manage grant over {instance_id}"));
            }
        }
    }
    let lifecycle = params["lifecycle"].as_str().ok_or("set_lifecycle.lifecycle required")?;
    if !matches!(lifecycle, "ACTIVE" | "PAUSED" | "PARKED" | "TERMINATED") {
        return Err(format!("unknown lifecycle {lifecycle:?}"));
    }
    let reason = params["reason"].as_str().unwrap_or("");
    let (session, epoch, current, _, _): (String, i64, String, String, i64) = load_instance(tx, instance_id)?;
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
    if lifecycle == "TERMINATED" {
        // termination settles everything the instance leaves behind (§5.4):
        // open execution of the epoch closes, its pending tasks cancel for
        // the requesters, and every grant it held or issued dies with it —
        // the member row is never just deleted
        let closed = close_epoch_execution(tx, session_id, instance_id, epoch, "instance terminated")?;
        let tasks = tx
            .execute(
                "UPDATE tasks SET status = 'CANCELLED', revision = revision + 1
                 WHERE assignee = ?1 AND session_id = ?2 AND status IN ('PENDING', 'RUNNING', 'BLOCKED')",
                rusqlite::params![instance_id, session_id],
            )
            .map_err(|e| format!("terminate tasks: {e}"))?;
        let mut revoked: Vec<String> = Vec::new();
        let held: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT id FROM grants WHERE subject = ?1 AND session_id = ?2 AND revoked_at IS NULL")
                .map_err(|e| format!("held grants: {e}"))?;
            let rows = stmt
                .query_map(rusqlite::params![instance_id, session_id], |row| row.get(0))
                .map_err(|e| format!("held grants query: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("held grants collect: {e}"))?
        };
        for id in held {
            revoked.extend(revoke_grant_tree(tx, &id)?);
        }
        let issued: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT id FROM grants WHERE issuer = ?1 AND session_id = ?2 AND revoked_at IS NULL")
                .map_err(|e| format!("issued grants: {e}"))?;
            let rows = stmt
                .query_map(rusqlite::params![instance_id, session_id], |row| row.get(0))
                .map_err(|e| format!("issued grants query: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("issued grants collect: {e}"))?
        };
        for id in issued {
            revoked.extend(revoke_grant_tree(tx, &id)?);
        }
        revoked.sort();
        revoked.dedup();
        if !revoked.is_empty() {
            bump_grant_revision(tx)?;
        }
        event(
            tx,
            session_id,
            "instance_terminated",
            instance_id,
            &json!({"closed": closed, "tasks_cancelled": tasks, "grants_revoked": revoked}),
        )?;
        return Ok(json!({"instance_id": instance_id, "lifecycle": lifecycle,
                         "closed": closed, "tasks_cancelled": tasks, "grants_revoked": revoked}));
    }
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

/// Shape check for goal-level required checks (§8): each check is a shell
/// command the runtime executes at the completion boundary, with an optional
/// timeout, network flag and declared inputs whose hashes bind the result.
fn validate_required_checks(checks: &Json) -> Result<(), String> {
    let list = checks.as_array().ok_or("limits.required_checks must be an array")?;
    for check in list {
        let id = check["id"].as_str().ok_or("limits.required_checks[].id required")?;
        let command = check["command"].as_str().ok_or("limits.required_checks[].command required")?;
        if id.is_empty() || command.is_empty() {
            return Err("limits.required_checks[] id/command must not be empty".into());
        }
        if let Some(timeout) = check.get("timeout") {
            if timeout.as_u64().is_none_or(|t| t == 0) {
                return Err("limits.required_checks[].timeout must be a positive number of seconds".into());
            }
        }
        if let Some(inputs) = check.get("inputs") {
            let paths = inputs.as_array().ok_or("limits.required_checks[].inputs must be an array of paths")?;
            for path in paths {
                let Some(text) = path.as_str() else {
                    return Err("limits.required_checks[].inputs must be an array of paths".into());
                };
                let relative = std::path::Path::new(text);
                if relative.is_absolute()
                    || relative.components().any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(
                        "limits.required_checks[].inputs must be workspace-relative paths without .. escapes".into()
                    );
                }
            }
        }
    }
    Ok(())
}

/// Required-check round registration (§8): the synthetic request, its
/// decision and one PREPARED shell operation per check commit in one
/// transaction, so check execution rides the same operation/receipt ledger
/// as model tools and the receipts auto-associate with the goal. Checks
/// enter only through create_goal limits (user-gated); the driver dispatches
/// them without an approval prompt because the user pre-authorized exactly
/// these commands — never because a model asked. The grant revision is
/// pinned to 0 like the driver's own registrations; the live capability
/// re-check at dispatch remains the real gate.
fn register_check_runs(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::System) {
        return Err("register_check_runs is driver-internal (system identity required)".into());
    }
    let goal_id = params["goal_id"].as_str().ok_or("register_check_runs.goal_id required")?;
    let instance_id = params["instance_id"].as_str().ok_or("register_check_runs.instance_id required")?;
    let round = params["round"].as_i64().ok_or("register_check_runs.round required")?;
    if round < 1 {
        return Err("register_check_runs.round must be >= 1".into());
    }
    let checks = params["checks"].as_array().cloned().unwrap_or_default();
    if checks.is_empty() {
        return Err("register_check_runs.checks must not be empty".into());
    }
    let goal_status: String = tx
        .query_row(
            "SELECT status FROM goals WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![goal_id, session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("register_check_runs goal {goal_id}: {e}"))?;
    if goal_status != "ACTIVE" {
        return Err(format!("goal {goal_id} is {goal_status}; not registering checks"));
    }
    let (session, epoch, _, phase, _) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if phase != "COMPLETION_PENDING" {
        return Err(format!("instance {instance_id} is {phase}; checks register only at the completion boundary"));
    }
    let request_id = format!("check:{goal_id}:{round}");
    let exists: bool = tx
        .query_row("SELECT COUNT(*) FROM model_requests WHERE request_id = ?1", [&request_id], |row| {
            row.get::<_, i64>(0)
        })
        .map(|n| n > 0)
        .map_err(|e| format!("check request read: {e}"))?;
    if exists {
        return Ok(json!({"decision_id": request_id, "round": round, "already_registered": true}));
    }
    tx.execute(
        "INSERT INTO model_requests (request_id, instance_id, epoch, goal_id, request_ref, selected_attempt_id, status)
         VALUES (?1, ?2, ?3, ?4, 'required_check', NULL, 'COMPLETE')",
        rusqlite::params![request_id, instance_id, epoch, goal_id],
    )
    .map_err(|e| format!("check request {request_id}: {e}"))?;
    tx.execute(
        "INSERT INTO decisions (decision_id, request_id, completion_json) VALUES (?1, ?2, NULL)",
        [&request_id, &request_id],
    )
    .map_err(|e| format!("check decision {request_id}: {e}"))?;
    // the assistant entry pairs the synthetic tool calls so strict wire
    // protocols stay valid once the receipts land as tool results (§8)
    let mut tool_calls = Vec::new();
    for (position, check) in checks.iter().enumerate() {
        let check_id = check["id"].as_str().ok_or("checks[].id required")?;
        let command_text = check["command"].as_str().ok_or("checks[].command required")?;
        let mut args = json!({"command": command_text, "check_id": check_id});
        if let Some(timeout) = check.get("timeout").and_then(Json::as_u64) {
            args["timeout"] = json!(timeout);
        }
        if check["network"].as_bool().unwrap_or(false) {
            args["network"] = json!(true);
        }
        if let Some(inputs) = check.get("inputs") {
            args["inputs"] = inputs.clone();
        }
        if let Some(observed) = check.get("inputs_observed") {
            args["inputs_observed"] = observed.clone();
        }
        // computed here from the stored args, never trusted from the caller
        let args_hash = crate::kernel::args_hash(&args);
        let call_id = format!("check-{round}-{position}");
        let fixed =
            json!({"index": position, "call_id": call_id, "name": "shell", "args": args, "args_hash": args_hash});
        tx.execute(
            "INSERT INTO operations
             (operation_id, decision_id, tool_index, goal_id, epoch, intent_json, args_hash, grant_revision, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 'PREPARED')",
            rusqlite::params![
                format!("{request_id}:{position}"),
                request_id,
                position as i64,
                goal_id,
                epoch,
                fixed.to_string(),
                args_hash
            ],
        )
        .map_err(|e| format!("check operation {request_id}:{position}: {e}"))?;
        tool_calls.push(json!({"id": call_id, "type": "function",
                               "function": {"name": "shell", "arguments": json!({"command": command_text}).to_string()}}));
    }
    append_context(
        tx,
        instance_id,
        epoch,
        "assistant",
        &json!({"role": "assistant",
                "content": format!("runtime required-check round {round} for goal {goal_id}"),
                "tool_calls": tool_calls}),
        Some(&format!("check-round-{goal_id}-{round}")),
        &[],
    )?;
    let summary: Vec<Json> = checks.iter().map(|c| json!({"id": c["id"], "command": c["command"]})).collect();
    event(
        tx,
        session_id,
        "check_round_registered",
        goal_id,
        &json!({"goal_id": goal_id, "round": round, "checks": summary}),
    )?;
    Ok(json!({"decision_id": request_id, "round": round, "operations": checks.len()}))
}

/// Check-failure repair path (§8): the failure receipts already sit in the
/// instance context (decision consumption), so the next request is a repair
/// turn that sees them; this only flips the completion boundary back to
/// READY — one transaction, replay-safe.
fn repair_completion(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::System) {
        return Err("repair_completion is driver-internal (system identity required)".into());
    }
    let instance_id = params["instance_id"].as_str().ok_or("repair_completion.instance_id required")?;
    let goal_id = params["goal_id"].as_str().ok_or("repair_completion.goal_id required")?;
    let round = params["round"].as_i64().unwrap_or(0);
    let failures = params.get("failures").cloned().unwrap_or(json!([]));
    let (session, _, _, phase, _) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if phase != "COMPLETION_PENDING" {
        return Ok(json!({"instance_id": instance_id, "phase": phase, "already_closed": true}));
    }
    tx.execute(
        "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'COMPLETION_PENDING'",
        [instance_id],
    )
    .map_err(|e| format!("repair completion: {e}"))?;
    event(
        tx,
        session_id,
        "completion_repair",
        instance_id,
        &json!({"goal_id": goal_id, "round": round, "failures": failures}),
    )?;
    Ok(json!({"instance_id": instance_id, "phase": "READY", "round": round}))
}

/// System-settled BLOCKED (§8): required checks exhausted their repair
/// rounds or the verification infrastructure itself failed. Never an
/// upgrade of the model's candidate — the distinct event keeps who decided
/// auditable.
fn block_goal(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::System) {
        return Err("block_goal is driver-internal (system identity required)".into());
    }
    let goal_id = params["goal_id"].as_str().ok_or("block_goal.goal_id required")?;
    let instance_id = params["instance_id"].as_str().ok_or("block_goal.instance_id required")?;
    let reason = params["reason"].as_str().unwrap_or("required checks did not pass");
    let goal_status: String = tx
        .query_row(
            "SELECT status FROM goals WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![goal_id, session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("block_goal {goal_id}: {e}"))?;
    if goal_status != "ACTIVE" {
        return Ok(json!({"goal_id": goal_id, "status": goal_status, "already_closed": true}));
    }
    tx.execute("UPDATE goals SET status = 'BLOCKED' WHERE id = ?1", [goal_id])
        .map_err(|e| format!("goal block: {e}"))?;
    tx.execute(
        "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'COMPLETION_PENDING'",
        [instance_id],
    )
    .map_err(|e| format!("block instance: {e}"))?;
    event(
        tx,
        session_id,
        "goal_blocked",
        goal_id,
        &json!({"goal_id": goal_id, "status": "BLOCKED", "reason": reason}),
    )?;
    Ok(json!({"goal_id": goal_id, "status": "BLOCKED"}))
}

/// End a completion turn with nothing to settle (§5.2): a finish from an
/// instance that owns no goal and holds no open assigned task simply
/// returns to READY. The stored candidate stays on its decision for audit.
fn close_completion(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let instance_id = params["instance_id"].as_str().ok_or("close_completion.instance_id required")?;
    let (session, _, _, phase, _) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if phase != "COMPLETION_PENDING" {
        return Ok(json!({"instance_id": instance_id, "phase": phase, "already_closed": true}));
    }
    tx.execute(
        "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'COMPLETION_PENDING'",
        [instance_id],
    )
    .map_err(|e| format!("close completion: {e}"))?;
    event(tx, session_id, "completion_closed", instance_id, &json!({"instance_id": instance_id}))?;
    Ok(json!({"instance_id": instance_id, "phase": "READY"}))
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
        ctl.submit(
            cmd(&format!("ci-{id}"), "create_instance", json!({"id": id, "workspace_ref": "/tmp/ws"})),
            Identity::User,
        )
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
                cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": false})),
                Identity::System,
            )
            .expect("dispatch");
        assert_eq!(ok["status"], json!("DISPATCH_COMMITTED"));
        // already dispatched: not dispatchable again
        let err = ctl
            .submit(
                cmd("dp-2", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": false})),
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
                cmd("dp-a", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": true})),
                Identity::System,
            )
            .expect("ask");
        assert_eq!(asked["status"], json!("APPROVAL_REQUIRED"));
        let approval_id = asked["approval_id"].as_str().unwrap().to_string();
        // re-dispatch returns the same pending approval, no duplicate row
        let again = ctl
            .submit(
                cmd("dp-b", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": true})),
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
                cmd("dp-c", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": true})),
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
                cmd("dp-a", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": true})),
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
        ctl.submit(cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-x:1"})), Identity::System)
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
    fn grants_narrow_only_and_parent_revocation_cascades() {
        let (mut ctl, path) = control("grants");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        // the user delegates session-wide management to i1 (Q5)
        let manage = ctl
            .submit(
                cmd("ig-m", "issue_grant", json!({"subject": "i1", "action": "manage", "resource_scope": "session"})),
                Identity::User,
            )
            .expect("issue manage");
        let manage_id = manage["grant_id"].as_str().unwrap().to_string();
        // a manage holder issues connection grants inside its scope
        let msg = ctl
            .submit(
                cmd(
                    "ig-c",
                    "issue_grant",
                    json!({"subject": "i2", "action": "message", "resource_scope": "instance:i1"}),
                ),
                Identity::Instance("i1".into()),
            )
            .expect("manage holder issues message");
        let msg_id = msg["grant_id"].as_str().unwrap().to_string();
        // no widening: i1's own shell grant is workspace-scoped, and manage
        // does not cover shell — shell@session is refused
        let err = ctl
            .submit(
                cmd("ig-w", "issue_grant", json!({"subject": "i2", "action": "shell", "resource_scope": "session"})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("holds no grant covering"), "{err}");
        // no widening: i2's message grant covers instance:i1, not the session
        let err = ctl
            .submit(
                cmd("ig-w2", "issue_grant", json!({"subject": "i1", "action": "message", "resource_scope": "session"})),
                Identity::Instance("i2".into()),
            )
            .unwrap_err();
        assert!(err.contains("holds no grant covering"), "{err}");
        // the system identity never issues grants
        assert!(ctl
            .submit(
                cmd("ig-sys", "issue_grant", json!({"subject": "i2", "action": "manage", "resource_scope": "session"})),
                Identity::System,
            )
            .is_err());
        // revoking the parent manage grant revokes the derived grant too (A03)
        let revoked = ctl
            .submit(cmd("rv-1", "revoke_grant", json!({"grant_id": manage_id})), Identity::User)
            .expect("revoke manage");
        let cascaded: Vec<String> =
            revoked["revoked"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert!(cascaded.contains(&manage_id) && cascaded.contains(&msg_id), "{cascaded:?}");
        let msg_revoked: Option<f64> = ctl
            .connection()
            .query_row("SELECT revoked_at FROM grants WHERE id = ?1", [&msg_id], |row| row.get(0))
            .unwrap();
        assert!(msg_revoked.is_some());
        // instances may revoke only grants they issued
        let err = ctl
            .submit(cmd("rv-2", "revoke_grant", json!({"grant_id": manage_id})), Identity::Instance("i2".into()))
            .unwrap_err();
        assert!(err.contains("issuing instance"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn revocation_blocks_queued_dispatch_until_reauthorized() {
        let (mut ctl, path) = control("reauth");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        let stamped: i64 = ctl
            .connection()
            .query_row("SELECT grant_revision FROM operations WHERE operation_id = 'd-x:0'", [], |row| row.get(0))
            .unwrap();
        // the user revokes the workspace shell grant while the op is queued
        let grant_id: String = ctl
            .connection()
            .query_row("SELECT id FROM grants WHERE subject = 'i1' AND action = 'shell'", [], |row| row.get(0))
            .unwrap();
        ctl.submit(cmd("rv-1", "revoke_grant", json!({"grant_id": grant_id})), Identity::User).expect("revoke");
        // default dispatch sees the revision bump and refuses (A04)
        let err = ctl
            .submit(cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-x:0"})), Identity::System)
            .unwrap_err();
        assert!(err.contains("refusing dispatch"), "{err}");
        // replaying the stamped revision reaches the capability re-check
        let err = ctl
            .submit(
                cmd("dp-2", "dispatch_operation", json!({"operation_id": "d-x:0", "permission_revision": stamped})),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("unauthorized"), "{err}");
        // re-authorization cancels the never-started op and wakes the decision
        let closed = ctl
            .submit(cmd("ra-1", "reauthorize_operation", json!({"operation_id": "d-x:0"})), Identity::System)
            .expect("reauthorize");
        assert_eq!(closed["status"], json!("CANCELLED"));
        assert_eq!(closed["decision_open"], json!(false));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        let message: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i1' AND kind = 'tool_result'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(message.contains("holds no shell@workspace grant"), "{message}");
        // a still-authorized op is re-stamped to the current revision instead
        ctl.submit(
            cmd("ig-1", "issue_grant", json!({"subject": "i1", "action": "shell", "resource_scope": "workspace"})),
            Identity::User,
        )
        .expect("re-issue shell");
        let revision: i64 =
            ctl.connection().query_row("SELECT revision FROM instances WHERE id = 'i1'", [], |row| row.get(0)).unwrap();
        open_decision(&mut ctl, "y", "i1", revision, 1);
        let re_stamped = ctl
            .submit(cmd("ra-2", "reauthorize_operation", json!({"operation_id": "d-y:0"})), Identity::System)
            .expect("reauthorize ok");
        assert_eq!(re_stamped["status"], json!("PREPARED"));
        let ok = ctl
            .submit(cmd("dp-3", "dispatch_operation", json!({"operation_id": "d-y:0"})), Identity::System)
            .expect("dispatch after re-stamp");
        assert_eq!(ok["status"], json!("DISPATCH_COMMITTED"));
        cleanup(&path);
    }

    #[test]
    fn late_receipt_after_reset_lands_on_the_old_epoch_only() {
        let (mut ctl, path) = control("reset-late");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        ctl.submit(cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-x:0"})), Identity::System)
            .expect("dispatch");
        // reset closes the epoch: the dispatched op keeps only a cancel flag
        let reset =
            ctl.submit(cmd("rs-1", "reset_instance", json!({"instance_id": "i1"})), Identity::User).expect("reset");
        assert_eq!(reset["epoch"], json!(1));
        let flagged: i64 = ctl
            .connection()
            .query_row("SELECT cancel_requested FROM operations WHERE operation_id = 'd-x:0'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(flagged, 1);
        // the late receipt still lands on the original operation (A24)…
        let done = ctl
            .submit(
                cmd(
                    "co-late",
                    "complete_operation",
                    json!({"operation_id": "d-x:0", "status": "SUCCEEDED",
                           "receipt": {"operation_id": "d-x:0", "ok": true, "content": "{\"output\":\"late\"}"}}),
                ),
                Identity::System,
            )
            .expect("late complete");
        assert_eq!(done["status"], json!("SUCCEEDED"));
        // …but it is audited in the old epoch and never enters the new one
        let old_epoch: i64 = ctl
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM context_entries WHERE instance_id = 'i1' AND epoch = 0 AND kind = 'tool_result'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_epoch, 1);
        let new_epoch: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM context_entries WHERE instance_id = 'i1' AND epoch = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(new_epoch, 0);
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        // the new epoch accepts fresh work immediately
        let revision: i64 =
            ctl.connection().query_row("SELECT revision FROM instances WHERE id = 'i1'", [], |row| row.get(0)).unwrap();
        open_decision(&mut ctl, "y", "i1", revision, 1);
        cleanup(&path);
    }

    #[test]
    fn reset_closes_epoch_and_releases_reservations() {
        let (mut ctl, path) = control("reset-close");
        create_instance(&mut ctl, "i1");
        ctl.submit(
            cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1", "limits": {"max_total_tokens": 1000}})),
            Identity::User,
        )
        .expect("goal");
        ctl.submit(
            cmd(
                "b-1",
                "begin_request",
                json!({"instance_id": "i1", "request_id": "r1", "revision": 1, "est_prompt_tokens": 800}),
            ),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        // the reservation of 800/1000 is held while the request is pending
        let held: String = ctl
            .connection()
            .query_row("SELECT reservations_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert!(held.contains("r1"), "{held}");
        // reset cancels the pending request and releases its reservation (§8)
        let reset =
            ctl.submit(cmd("rs-1", "reset_instance", json!({"instance_id": "i1"})), Identity::User).expect("reset");
        assert_eq!(reset["closed"]["requests_closed"], json!(1));
        let status: String = ctl
            .connection()
            .query_row("SELECT status FROM model_requests WHERE request_id = 'r1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "CANCELLED");
        let reservations: String = ctl
            .connection()
            .query_row("SELECT reservations_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(reservations, "{}");
        // budget is whole again: the same estimate fits in the new epoch
        let revision: i64 =
            ctl.connection().query_row("SELECT revision FROM instances WHERE id = 'i1'", [], |row| row.get(0)).unwrap();
        let ok = ctl
            .submit(
                cmd(
                    "b-3",
                    "begin_request",
                    json!({"instance_id": "i1", "request_id": "r3", "revision": revision, "est_prompt_tokens": 800}),
                ),
                Identity::Instance("i1".into()),
            )
            .expect("begin r3");
        assert_eq!(ok["phase"], json!("MODEL_PENDING"));
        // a stale request id can never be imported after the reset
        let err = ctl
            .submit(
                cmd(
                    "imp-stale",
                    "import_response",
                    json!({"request_id": "r1", "decision_id": "d-stale",
                           "entry": {"role": "assistant", "content": "late"}, "intents": []}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("r1"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn terminate_closes_tasks_grants_and_execution() {
        let (mut ctl, path) = control("terminate");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        // i2 manages the session and derived a connection grant to i1
        ctl.submit(
            cmd("ig-m", "issue_grant", json!({"subject": "i2", "action": "manage", "resource_scope": "session"})),
            Identity::User,
        )
        .expect("issue manage");
        let derived = ctl
            .submit(
                cmd(
                    "ig-c",
                    "issue_grant",
                    json!({"subject": "i1", "action": "message", "resource_scope": "instance:i2"}),
                ),
                Identity::Instance("i2".into()),
            )
            .expect("derived grant");
        let derived_id = derived["grant_id"].as_str().unwrap().to_string();
        // a running task assigned to i2 and a queued operation
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.connection()
            .execute(
                "INSERT INTO tasks (id, goal_id, session_id, requester, assignee, dependencies_json,
                                    acceptance_refs_json, status, result_refs_json, revision)
                 VALUES ('t1', 'g1', 's1', 'user', 'i2', '[]', '[]', 'RUNNING', '[]', 0)",
                [],
            )
            .unwrap();
        open_decision(&mut ctl, "x", "i2", 0, 1);
        // terminate settles everything in one transaction (§5.4)
        let done = ctl
            .submit(
                cmd("sl-t", "set_lifecycle", json!({"instance_id": "i2", "lifecycle": "TERMINATED"})),
                Identity::User,
            )
            .expect("terminate");
        assert_eq!(done["tasks_cancelled"], json!(1));
        let task: String =
            ctl.connection().query_row("SELECT status FROM tasks WHERE id = 't1'", [], |row| row.get(0)).unwrap();
        assert_eq!(task, "CANCELLED");
        // the queued op cancelled before any side effect
        let op: String = ctl
            .connection()
            .query_row("SELECT status FROM operations WHERE operation_id = 'd-x:0'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(op, "CANCELLED");
        // every grant held or issued by i2 is gone, derived ones included
        let active: i64 = ctl
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE (subject = 'i2' OR issuer = 'i2') AND revoked_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
        let derived_revoked: Option<f64> = ctl
            .connection()
            .query_row("SELECT revoked_at FROM grants WHERE id = ?1", [&derived_id], |row| row.get(0))
            .unwrap();
        assert!(derived_revoked.is_some());
        // i1 itself is untouched: its own workspace grant survives
        let i1_grants: i64 = ctl
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE subject = 'i1' AND action = 'shell' AND revoked_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(i1_grants, 1);
        cleanup(&path);
    }

    /// Give `subject` a message grant over `instance:<peer>` issued by the
    /// user — the edges of the communication graph (§5.1).
    fn grant_message(ctl: &mut Control, subject: &str, peer: &str, tag: &str) {
        ctl.submit(
            cmd(
                &format!("gm-{tag}"),
                "issue_grant",
                json!({"subject": subject, "action": "message", "resource_scope": format!("instance:{peer}")}),
            ),
            Identity::User,
        )
        .expect("grant message");
    }

    fn drain(ctl: &mut Control, instance: &str) -> Json {
        ctl.submit(
            cmd(&format!("dr-{instance}-{}", uuid::Uuid::new_v4()), "drain_inbox", json!({"instance_id": instance})),
            Identity::Instance(instance.into()),
        )
        .expect("drain")
    }

    #[test]
    fn messages_flow_across_an_authorized_ring() {
        let (mut ctl, path) = control("ring");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        create_instance(&mut ctl, "i3");
        // A02: i1→i2→i3→i1, every edge an explicit grant
        grant_message(&mut ctl, "i1", "i2", "a");
        grant_message(&mut ctl, "i2", "i3", "b");
        grant_message(&mut ctl, "i3", "i1", "c");
        for (from, to, text) in [("i1", "i2", "ping-b"), ("i2", "i3", "ping-c"), ("i3", "i1", "ping-a")] {
            ctl.submit(
                cmd(&format!("sm-{from}"), "send_message", json!({"recipient": to, "text": text})),
                Identity::Instance(from.into()),
            )
            .expect("send");
        }
        // queued, not yet in any context: receiving never wakes a turn (§5.3)
        for instance in ["i1", "i2", "i3"] {
            assert_eq!(context_count(&ctl, instance), 0);
        }
        let drained = drain(&mut ctl, "i2");
        assert_eq!(drained["applied"], json!(1));
        let message: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i2' AND kind = 'user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(message.contains("[message from i1] ping-b"), "{message}");
        // a replayed drain does not re-apply (A06)
        let again = drain(&mut ctl, "i2");
        assert_eq!(again["applied"], json!(0));
        assert_eq!(context_count(&ctl, "i2"), 1);
        cleanup(&path);
    }

    #[test]
    fn send_message_requires_grant_and_backpressure_is_explicit() {
        let (mut ctl, path) = control("backpressure");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        // no grant: the send fails closed
        let err = ctl
            .submit(
                cmd("sm-0", "send_message", json!({"recipient": "i2", "text": "hi"})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("message grant"), "{err}");
        grant_message(&mut ctl, "i1", "i2", "a");
        ctl.submit(
            cmd("sm-1", "send_message", json!({"recipient": "i2", "text": "one", "max_inbox": 1})),
            Identity::Instance("i1".into()),
        )
        .expect("first send");
        // the second send sees the full inbox and fails openly (§5.3)
        let err = ctl
            .submit(
                cmd("sm-2", "send_message", json!({"recipient": "i2", "text": "two", "max_inbox": 1})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("backpressure"), "{err}");
        // draining frees the inbox; nothing was silently dropped
        drain(&mut ctl, "i2");
        ctl.submit(
            cmd("sm-3", "send_message", json!({"recipient": "i2", "text": "three", "max_inbox": 1})),
            Identity::Instance("i1".into()),
        )
        .expect("send after drain");
        cleanup(&path);
    }

    #[test]
    fn delegate_and_complete_task_uses_the_narrow_return_path() {
        let (mut ctl, path) = control("delegate");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        // unknown and self dependencies are refused (§5.3 acyclic prerequisites)
        let err = ctl
            .submit(
                cmd(
                    "dt-bad",
                    "delegate_task",
                    json!({"task_id": "t-bad", "assignee": "i2", "goal_id": "g1", "dependencies": ["t-missing"]}),
                ),
                Identity::User,
            )
            .unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        // i1 needs a delegate grant over i2 to assign work (Q5)
        let err = ctl
            .submit(
                cmd(
                    "dt-ng",
                    "delegate_task",
                    json!({"task_id": "t0", "assignee": "i2", "goal_id": "g1", "description": "nope"}),
                ),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("delegate grant"), "{err}");
        ctl.submit(
            cmd("gd-1", "issue_grant", json!({"subject": "i1", "action": "delegate", "resource_scope": "instance:i2"})),
            Identity::User,
        )
        .expect("grant delegate");
        let delegated = ctl
            .submit(
                cmd(
                    "dt-1",
                    "delegate_task",
                    json!({"task_id": "t1", "assignee": "i2", "goal_id": "g1",
                           "description": "summarise the report", "acceptance_refs": ["spec:1"]}),
                ),
                Identity::Instance("i1".into()),
            )
            .expect("delegate");
        assert!(delegated["return_grant"].as_str().is_some());
        // the assignment queues for i2 and applies at its boundary
        let drained = drain(&mut ctl, "i2");
        assert_eq!(drained["applied"], json!(1));
        let message: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i2' AND kind = 'user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(message.contains("[task t1 assigned by i1] summarise the report"), "{message}");
        ctl.submit(cmd("st-1", "start_task", json!({"task_id": "t1"})), Identity::Instance("i2".into()))
            .expect("start");
        // settlement delivers the result to the requester and consumes the
        // return grant in the same transaction (§5.3)
        let done = ctl
            .submit(
                cmd(
                    "ct-1",
                    "complete_task",
                    json!({"task_id": "t1", "status": "SUCCEEDED", "summary": "done", "result_refs": ["art:9"]}),
                ),
                Identity::Instance("i2".into()),
            )
            .expect("complete");
        assert_eq!(done["delivered"], json!(true));
        let return_grants: i64 = ctl
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE action = 'task_result' AND resource_scope = 'task:t1'
                 AND revoked_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(return_grants, 0);
        // i1 learns the result at its own boundary
        let drained = drain(&mut ctl, "i1");
        assert_eq!(drained["applied"], json!(1));
        let message: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i1' AND kind = 'user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(message.contains("[task t1 result from i2]"), "{message}");
        // the return path is gone: no second settlement, no new channel
        let replay = ctl
            .submit(
                cmd("ct-2", "complete_task", json!({"task_id": "t1", "status": "SUCCEEDED", "summary": "done"})),
                Identity::Instance("i2".into()),
            )
            .expect("replay");
        assert_eq!(replay["replayed"], json!(true));
        let err = ctl
            .submit(
                cmd("ct-3", "complete_task", json!({"task_id": "t1", "status": "FAILED", "summary": "changed mind"})),
                Identity::Instance("i2".into()),
            )
            .unwrap_err();
        assert!(err.contains("already SUCCEEDED"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn cancel_task_severs_the_return_path() {
        let (mut ctl, path) = control("cancel-task");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate");
        ctl.submit(cmd("ct-x", "cancel_task", json!({"task_id": "t1", "reason": "superseded"})), Identity::User)
            .expect("cancel");
        // the assignee is informed at its boundary (assignment + cancel);
        // settlement is refused
        let drained = drain(&mut ctl, "i2");
        assert_eq!(drained["applied"], json!(2));
        let err = ctl
            .submit(
                cmd("ct-1", "complete_task", json!({"task_id": "t1", "status": "SUCCEEDED", "summary": "late"})),
                Identity::Instance("i2".into()),
            )
            .unwrap_err();
        assert!(err.contains("task_result grant") || err.contains("already CANCELLED"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn read_history_is_user_or_self_only() {
        let (mut ctl, path) = control("history");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        ctl.submit(
            cmd("in-1", "submit_input", json!({"instance_id": "i1", "envelope_id": "e1", "text": "secret"})),
            Identity::User,
        )
        .expect("input");
        // A05: another instance is refused at the controlled entry
        let err = ctl
            .submit(cmd("rh-0", "read_history", json!({"instance_id": "i1"})), Identity::Instance("i2".into()))
            .unwrap_err();
        assert!(err.contains("private history"), "{err}");
        // self and the user read fine
        let own = ctl
            .submit(cmd("rh-1", "read_history", json!({"instance_id": "i2"})), Identity::Instance("i2".into()))
            .expect("own history");
        assert_eq!(own["entries"].as_array().unwrap().len(), 0);
        let full = ctl.submit(cmd("rh-2", "read_history", json!({"instance_id": "i1"})), Identity::User).expect("user");
        assert_eq!(full["entries"].as_array().unwrap().len(), 1);
        cleanup(&path);
    }

    /// Drive one instance into WAITING via a response that registers a
    /// wait; returns the wait id (`w-d-{tag}`).
    fn open_wait(ctl: &mut Control, tag: &str, instance: &str, revision: i64, wait: Json) -> String {
        let request = begin_and_complete(ctl, tag, instance, revision);
        ctl.submit(
            cmd(
                &format!("imp-{tag}"),
                "import_response",
                json!({"request_id": request, "decision_id": format!("d-{tag}"),
                       "entry": {"role": "assistant", "content": "waiting"}, "intents": [], "wait": wait}),
            ),
            Identity::System,
        )
        .expect("import wait");
        format!("w-d-{tag}")
    }

    fn wait_state(ctl: &Control, wait_id: &str) -> String {
        ctl.connection().query_row("SELECT status FROM waits WHERE id = ?1", [wait_id], |row| row.get(0)).unwrap()
    }

    #[test]
    fn wait_for_an_arrived_result_is_satisfied_at_registration() {
        let (mut ctl, path) = control("wait-late");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        grant_message(&mut ctl, "i2", "i1", "a");
        // the result arrives BEFORE the wait is registered (A23)
        ctl.submit(
            cmd("sm-1", "send_message", json!({"recipient": "i1", "text": "done"})),
            Identity::Instance("i2".into()),
        )
        .expect("send");
        drain(&mut ctl, "i1");
        let imported = ctl
            .submit(
                cmd("b-w", "begin_request", json!({"instance_id": "i1", "request_id": "r-w", "revision": 1})),
                Identity::Instance("i1".into()),
            )
            .expect("begin");
        assert_eq!(imported["phase"], json!("MODEL_PENDING"));
        ctl.submit(
            cmd(
                "a-w",
                "record_attempt",
                json!({"attempt_id": "at-w", "request_id": "r-w", "status": "COMPLETE",
                       "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
            ),
            Identity::System,
        )
        .expect("attempt");
        let imported = ctl
            .submit(
                cmd(
                    "imp-w",
                    "import_response",
                    json!({"request_id": "r-w", "decision_id": "d-w",
                           "entry": {"role": "assistant", "content": "waiting"}, "intents": [],
                           "wait": {"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}]}}),
                ),
                Identity::System,
            )
            .expect("import");
        // satisfied at registration: the instance never parks (A23)
        assert_eq!(imported["wait"]["satisfied"], json!(true));
        assert_eq!(imported["phase"], json!("READY"));
        assert_eq!(wait_state(&ctl, "w-d-w"), "SATISFIED");
        cleanup(&path);
    }

    #[test]
    fn pending_wait_wakes_in_the_same_transaction_as_the_fact() {
        let (mut ctl, path) = control("wait-wake");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        grant_message(&mut ctl, "i2", "i1", "a");
        let wait_id = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}]}),
        );
        assert_eq!(phase_of(&ctl, "i1"), "WAITING");
        assert_eq!(wait_state(&ctl, &wait_id), "PENDING");
        // the message lands; the drain that applies it wakes the wait in the
        // same transaction — no polling, no lost wake (A23)
        ctl.submit(
            cmd("sm-1", "send_message", json!({"recipient": "i1", "text": "result"})),
            Identity::Instance("i2".into()),
        )
        .expect("send");
        let drained = drain(&mut ctl, "i1");
        assert_eq!(drained["woken"], json!([wait_id.clone()]));
        assert_eq!(wait_state(&ctl, &wait_id), "SATISFIED");
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn task_completion_wakes_the_waiter() {
        let (mut ctl, path) = control("wait-task");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate");
        let wait_id = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "task", "task_id": "t1"}]}),
        );
        assert_eq!(phase_of(&ctl, "i1"), "WAITING");
        let done = ctl
            .submit(
                cmd("ct-1", "complete_task", json!({"task_id": "t1", "status": "SUCCEEDED", "summary": "ok"})),
                Identity::Instance("i2".into()),
            )
            .expect("complete");
        assert_eq!(done["woken"], json!([wait_id.clone()]));
        assert_eq!(wait_state(&ctl, &wait_id), "SATISFIED");
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn task_settlement_closes_the_turn_and_notes_queue_continuation() {
        let (mut ctl, path) = control("settle-note");
        create_instance(&mut ctl, "i2");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate t1");
        ctl.submit(
            cmd("dt-2", "delegate_task", json!({"task_id": "t2", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate t2");
        // i2's finish turn parks it in COMPLETION_PENDING
        let request = begin_and_complete(&mut ctl, "fin", "i2", 0);
        ctl.submit(
            cmd(
                "imp-fin",
                "import_response",
                json!({"request_id": request, "decision_id": "d-fin",
                       "entry": {"role": "assistant", "content": "done"},
                       "completion": {"outcome": "success", "summary": "t1 done"}}),
            ),
            Identity::System,
        )
        .expect("import");
        assert_eq!(phase_of(&ctl, "i2"), "COMPLETION_PENDING");
        // settling t1 closes the turn and, with t2 still open, notes the
        // remaining queue so the loop does not park on an assistant tail
        ctl.submit(
            cmd("ct-1", "complete_task", json!({"task_id": "t1", "status": "SUCCEEDED", "summary": "t1 done"})),
            Identity::Instance("i2".into()),
        )
        .expect("complete t1");
        assert_eq!(phase_of(&ctl, "i2"), "READY");
        let note: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i2' ORDER BY idx DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(note.contains("settled: SUCCEEDED") && note.contains("1 open task(s)"), "{note}");
        // settling the last open task closes the turn without a continuation note
        let revision: i64 =
            ctl.connection().query_row("SELECT revision FROM instances WHERE id = 'i2'", [], |row| row.get(0)).unwrap();
        let request = begin_and_complete(&mut ctl, "fin2", "i2", revision);
        ctl.submit(
            cmd(
                "imp-fin2",
                "import_response",
                json!({"request_id": request, "decision_id": "d-fin2",
                       "entry": {"role": "assistant", "content": "done again"},
                       "completion": {"outcome": "success", "summary": "t2 done"}}),
            ),
            Identity::System,
        )
        .expect("import 2");
        let before = context_count(&ctl, "i2");
        ctl.submit(
            cmd("ct-2", "complete_task", json!({"task_id": "t2", "status": "SUCCEEDED", "summary": "t2 done"})),
            Identity::Instance("i2".into()),
        )
        .expect("complete t2");
        assert_eq!(phase_of(&ctl, "i2"), "READY");
        assert_eq!(context_count(&ctl, "i2"), before, "an empty queue needs no continuation note");
        cleanup(&path);
    }

    #[test]
    fn settling_a_task_mid_turn_leaves_the_live_phase_alone() {
        let (mut ctl, path) = control("settle-mid-turn");
        create_instance(&mut ctl, "i2");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate");
        ctl.submit(
            cmd("b-1", "begin_request", json!({"instance_id": "i2", "request_id": "r1", "revision": 0})),
            Identity::Instance("i2".into()),
        )
        .expect("begin");
        assert_eq!(phase_of(&ctl, "i2"), "MODEL_PENDING");
        // a user-side settlement never rewires a live phase nor notes the queue
        ctl.submit(cmd("ct-1", "complete_task", json!({"task_id": "t1", "status": "SUCCEEDED"})), Identity::User)
            .expect("settle");
        assert_eq!(phase_of(&ctl, "i2"), "MODEL_PENDING");
        assert_eq!(context_count(&ctl, "i2"), 0);
        cleanup(&path);
    }

    #[test]
    fn close_completion_returns_to_ready_without_a_settlement() {
        let (mut ctl, path) = control("close-completion");
        create_instance(&mut ctl, "i1");
        // a finish from an instance with no goal and no tasks parks in
        // COMPLETION_PENDING; nothing needs settling
        let request = begin_and_complete(&mut ctl, "fin", "i1", 0);
        ctl.submit(
            cmd(
                "imp-fin",
                "import_response",
                json!({"request_id": request, "decision_id": "d-fin",
                       "entry": {"role": "assistant", "content": "idle"},
                       "completion": {"outcome": "success", "summary": "nothing to do"}}),
            ),
            Identity::System,
        )
        .expect("import");
        assert_eq!(phase_of(&ctl, "i1"), "COMPLETION_PENDING");
        let closed =
            ctl.submit(cmd("cc-1", "close_completion", json!({"instance_id": "i1"})), Identity::System).expect("close");
        assert_eq!(closed["phase"], json!("READY"));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        // a replayed close is idempotent
        let again = ctl
            .submit(cmd("cc-2", "close_completion", json!({"instance_id": "i1"})), Identity::System)
            .expect("reclose");
        assert_eq!(again["already_closed"], json!(true));
        cleanup(&path);
    }

    #[test]
    fn a_worker_shares_the_budget_of_the_goal_its_queue_serves() {
        let (mut ctl, path) = control("a18-budget");
        create_instance(&mut ctl, "i-leader");
        create_instance(&mut ctl, "i-worker");
        ctl.submit(
            cmd(
                "g1",
                "create_goal",
                json!({"id": "g1", "instance_id": "i-leader", "limits": {"max_total_tokens": 100}}),
            ),
            Identity::User,
        )
        .expect("goal");
        ctl.submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i-worker", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate");
        // the goal-less worker reserves against the shared goal (A18)
        let begun = ctl
            .submit(
                cmd(
                    "b-1",
                    "begin_request",
                    json!({"instance_id": "i-worker", "request_id": "r1", "revision": 0, "est_prompt_tokens": 40}),
                ),
                Identity::Instance("i-worker".into()),
            )
            .expect("begin");
        assert!(begun.get("budget_refused").is_none(), "{begun}");
        let reservations: String = ctl
            .connection()
            .query_row("SELECT reservations_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert!(reservations.contains("r1"), "the worker reservation is visible on the goal: {reservations}");
        // the worker's completed attempt bills to the shared goal
        ctl.submit(
            cmd(
                "a-1",
                "record_attempt",
                json!({"attempt_id": "at-1", "request_id": "r1", "status": "COMPLETE",
                       "usage": {"prompt_tokens": 30, "completion_tokens": 15, "total_tokens": 45}}),
            ),
            Identity::System,
        )
        .expect("attempt");
        ctl.submit(
            cmd(
                "imp-1",
                "import_response",
                json!({"request_id": "r1", "decision_id": "d-1", "entry": {"role": "assistant", "content": "partial"}}),
            ),
            Identity::System,
        )
        .expect("import");
        let known: String = ctl
            .connection()
            .query_row("SELECT known_usage_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert!(known.contains("45"), "worker usage billed to the shared goal: {known}");
        // 45 known + 60 estimated > 100: the shared gate refuses the worker…
        let refused = ctl
            .submit(
                cmd(
                    "b-2",
                    "begin_request",
                    json!({"instance_id": "i-worker", "request_id": "r2", "revision": 2, "est_prompt_tokens": 60}),
                ),
                Identity::Instance("i-worker".into()),
            )
            .expect("refused begin");
        assert_eq!(refused["budget_refused"], json!(true));
        assert_eq!(phase_of(&ctl, "i-worker"), "READY");
        // …and the leader alike: one budget governs every instance (A18)
        let refused = ctl
            .submit(
                cmd(
                    "b-3",
                    "begin_request",
                    json!({"instance_id": "i-leader", "request_id": "r3", "revision": 1, "est_prompt_tokens": 60}),
                ),
                Identity::Instance("i-leader".into()),
            )
            .expect("leader refused begin");
        assert_eq!(refused["budget_refused"], json!(true));
        cleanup(&path);
    }

    #[test]
    fn a_due_timer_closes_the_wait() {
        let (mut ctl, path) = control("wait-timer");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        let future = crate::models::now() + 3600.0;
        let wait_id = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}], "timer_at": future}),
        );
        assert_eq!(phase_of(&ctl, "i1"), "WAITING");
        // not yet due: the timer pass leaves it pending
        let early = ctl.submit(cmd("ft-0", "fire_timer", json!({})), Identity::System).expect("early fire");
        assert_eq!(early["satisfied"], json!([]));
        // past the deadline the timer branch closes the wait (§5.3)
        let fired =
            ctl.submit(cmd("ft-1", "fire_timer", json!({"now": future + 1.0})), Identity::System).expect("fire");
        assert_eq!(fired["satisfied"], json!([wait_id.clone()]));
        assert_eq!(wait_state(&ctl, &wait_id), "SATISFIED");
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn blocked_report_flags_dead_waits_not_cycles() {
        let (mut ctl, path) = control("wait-blocked");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        create_instance(&mut ctl, "i3");
        create_instance(&mut ctl, "i4");
        let dead_wait = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}]}),
        );
        // an ordinary wait cycle among live instances is not a deadlock (A22)
        open_wait(&mut ctl, "y", "i3", 0, json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i4"}]}));
        open_wait(&mut ctl, "z", "i4", 0, json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i3"}]}));
        let report = ctl.submit(cmd("br-0", "blocked_report", json!({})), Identity::User).expect("report");
        assert_eq!(report["blocked"], json!([]));
        assert_eq!(report["waiting"], json!(3));
        // the sender dies: the wait can never fire and is reported — but
        // nothing is cancelled automatically (A22)
        ctl.submit(
            cmd("sl-t", "set_lifecycle", json!({"instance_id": "i2", "lifecycle": "TERMINATED"})),
            Identity::User,
        )
        .expect("terminate");
        let report = ctl.submit(cmd("br-1", "blocked_report", json!({})), Identity::User).expect("report");
        let blocked = report["blocked"].as_array().unwrap();
        assert_eq!(blocked.len(), 1, "{blocked:?}");
        assert_eq!(blocked[0]["wait_id"], json!(dead_wait.clone()));
        assert_eq!(report["waiting"], json!(2));
        assert_eq!(wait_state(&ctl, &dead_wait), "PENDING");
        assert_eq!(phase_of(&ctl, "i1"), "WAITING");
        cleanup(&path);
    }

    #[test]
    fn user_input_supersedes_a_pending_wait() {
        let (mut ctl, path) = control("wait-input");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        let wait_id = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}]}),
        );
        assert_eq!(phase_of(&ctl, "i1"), "WAITING");
        ctl.submit(
            cmd("in-1", "submit_input", json!({"instance_id": "i1", "envelope_id": "e1", "text": "stop waiting"})),
            Identity::User,
        )
        .expect("input");
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        assert_eq!(wait_state(&ctl, &wait_id), "CANCELLED");
        cleanup(&path);
    }

    #[test]
    fn spawn_requires_a_manage_grant() {
        let (mut ctl, path) = control("spawn");
        create_instance(&mut ctl, "leader");
        // without a manage grant the spawn fails closed (§5.1)
        let err = ctl
            .submit(
                cmd("sp-0", "spawn_instance", json!({"instance_id": "w1", "instructions": "work", "task": "dig"})),
                Identity::Instance("leader".into()),
            )
            .unwrap_err();
        assert!(err.contains("manage grant"), "{err}");
        let missing: i64 =
            ctl.connection().query_row("SELECT COUNT(*) FROM instances WHERE id = 'w1'", [], |row| row.get(0)).unwrap();
        assert_eq!(missing, 0);
        cleanup(&path);
    }

    #[test]
    fn spawn_registers_everything_in_one_transaction() {
        let (mut ctl, path) = control("spawn-ok");
        create_instance(&mut ctl, "leader");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.submit(
            cmd("mg-1", "issue_grant", json!({"subject": "leader", "action": "manage", "resource_scope": "session"})),
            Identity::User,
        )
        .expect("manage");
        let spawned = ctl
            .submit(
                cmd(
                    "sp-1",
                    "spawn_instance",
                    json!({"instance_id": "w1", "instructions": "work", "task": "dig", "task_id": "t-w1", "goal_id": "g1"}),
                ),
                Identity::Instance("leader".into()),
            )
            .expect("spawn");
        assert_eq!(spawned["instance_id"], json!("w1"));
        // instance exists and holds no automatic shell grant (§5.1)
        assert_eq!(phase_of(&ctl, "w1"), "READY");
        let shell_grants: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM grants WHERE subject = 'w1' AND action = 'shell'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(shell_grants, 0);
        // the task is registered with its narrow return path (§5.2/§5.3)
        let task: (String, String) = ctl
            .connection()
            .query_row("SELECT requester, assignee FROM tasks WHERE id = 't-w1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(task, ("leader".to_string(), "w1".to_string()));
        let return_grants: i64 = ctl
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE subject = 'w1' AND action = 'task_result'
                 AND resource_scope = 'task:t-w1' AND revoked_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(return_grants, 1);
        // the assignment waits in w1's inbox for its boundary
        let queued: i64 = ctl
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM envelopes WHERE recipient = 'w1' AND kind = 'task_assigned' AND state = 'ACCEPTED'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued, 1);
        cleanup(&path);
    }

    #[test]
    fn collaboration_intents_are_rechecked_at_dispatch() {
        let (mut ctl, path) = control("collab-dispatch");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        // a send intent without a message grant: refused at the linearization
        // point even though the op was imported while the schema was visible
        let request = begin_and_complete(&mut ctl, "x", "i1", 0);
        ctl.submit(
            cmd(
                "imp-x",
                "import_response",
                json!({"request_id": request, "decision_id": "d-x",
                       "entry": {"role": "assistant", "content": "send"},
                       "intents": [{"index": 0, "call_id": "c0", "name": "send",
                                    "args": {"recipient": "i2", "text": "hi"}}]}),
            ),
            Identity::System,
        )
        .expect("import");
        let err = ctl
            .submit(cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-x:0"})), Identity::System)
            .unwrap_err();
        assert!(err.contains("message grant"), "{err}");
        // the grant arrives; re-authorization re-stamps and dispatch passes
        grant_message(&mut ctl, "i1", "i2", "a");
        let re = ctl
            .submit(cmd("ra-1", "reauthorize_operation", json!({"operation_id": "d-x:0"})), Identity::System)
            .expect("reauthorize");
        assert_eq!(re["status"], json!("PREPARED"));
        let ok = ctl
            .submit(cmd("dp-2", "dispatch_operation", json!({"operation_id": "d-x:0"})), Identity::System)
            .expect("dispatch");
        assert_eq!(ok["status"], json!("DISPATCH_COMMITTED"));
        cleanup(&path);
    }

    #[test]
    fn a_relative_timer_closes_the_wait() {
        let (mut ctl, path) = control("wait-relative");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        let wait_id = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}], "timer_seconds": 3600}),
        );
        assert_eq!(phase_of(&ctl, "i1"), "WAITING");
        // the stored deadline is absolute, computed from the import clock
        let timer_at: f64 = ctl
            .connection()
            .query_row("SELECT timer_at FROM waits WHERE id = ?1", [&wait_id], |row| row.get(0))
            .unwrap();
        assert!(timer_at > crate::models::now(), "{timer_at}");
        let fired =
            ctl.submit(cmd("ft-1", "fire_timer", json!({"now": timer_at + 1.0})), Identity::System).expect("fire");
        assert_eq!(fired["satisfied"], json!([wait_id.clone()]));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn the_wake_reason_joins_the_context() {
        let (mut ctl, path) = control("wake-note");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        grant_message(&mut ctl, "i2", "i1", "a");
        let wait_id = open_wait(
            &mut ctl,
            "x",
            "i1",
            0,
            json!({"mode": "ANY", "conditions": [{"kind": "message", "from": "i2"}]}),
        );
        ctl.submit(
            cmd("sm-1", "send_message", json!({"recipient": "i1", "text": "ping"})),
            Identity::Instance("i2".into()),
        )
        .expect("send");
        drain(&mut ctl, "i1");
        assert_eq!(wait_state(&ctl, &wait_id), "SATISFIED");
        // the model sees why it woke; the note lands after the message
        let note: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i1' AND kind = 'note' ORDER BY idx DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(note.contains(&format!("[wait {wait_id} satisfied]")), "{note}");
        // a replayed wake pass does not duplicate the note
        let notes: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM context_entries WHERE instance_id = 'i1' AND kind = 'note'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(notes, 1);
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
    fn unknown_outcome_parks_running_tasks_and_notifies() {
        let (mut ctl, path) = control("unknown-park");
        create_instance(&mut ctl, "i1");
        create_instance(&mut ctl, "i2");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1"})), Identity::User).expect("goal");
        ctl.submit(
            cmd("dt-1", "delegate_task", json!({"task_id": "t1", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate");
        ctl.submit(
            cmd("dt-2", "delegate_task", json!({"task_id": "t2", "assignee": "i2", "goal_id": "g1"})),
            Identity::User,
        )
        .expect("delegate 2");
        ctl.submit(cmd("st-1", "start_task", json!({"task_id": "t1"})), Identity::Instance("i2".into()))
            .expect("start");
        open_decision(&mut ctl, "u", "i2", 0, 1);
        let done = ctl
            .submit(
                cmd(
                    "co-u",
                    "complete_operation",
                    json!({"operation_id": "d-u:0", "status": "OUTCOME_UNKNOWN",
                           "receipt": {"name": "shell", "started": true,
                                       "error": {"class": "outcome_unknown", "message": "runner crashed"}}}),
                ),
                Identity::System,
            )
            .expect("complete");
        assert_eq!(done["status"], json!("OUTCOME_UNKNOWN"));
        // §6.3/A09: the running task parks and the event notifies; the
        // pending queue (independent work) is untouched
        let status = |id: &str| -> String {
            ctl.connection().query_row("SELECT status FROM tasks WHERE id = ?1", [id], |row| row.get(0)).unwrap()
        };
        assert_eq!(status("t1"), "BLOCKED");
        assert_eq!(status("t2"), "PENDING");
        let blocked_events: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'task_blocked' AND scope = 't1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(blocked_events, 1);
        // a replayed completion returns the stored state without re-parking
        let replay = ctl
            .submit(
                cmd(
                    "co-u2",
                    "complete_operation",
                    json!({"operation_id": "d-u:0", "status": "OUTCOME_UNKNOWN",
                           "receipt": {"name": "shell", "started": true,
                                       "error": {"class": "outcome_unknown", "message": "runner crashed"}}}),
                ),
                Identity::System,
            )
            .expect("replay");
        assert_eq!(replay["replayed"], json!(true));
        let blocked_events: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'task_blocked'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(blocked_events, 1);
        drop(ctl);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn goal_deadline_refuses_new_requests_and_dispatches() {
        let (mut ctl, path) = control("goal-deadline");
        create_instance(&mut ctl, "i1");
        let past = crate::models::now() - 100.0;
        ctl.submit(
            cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1", "deadline": past})),
            Identity::User,
        )
        .expect("goal");
        // A35: past the deadline no new request registers; the refusal is a
        // committed, auditable outcome and the instance stays READY
        let refused = ctl
            .submit(
                cmd("b-1", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 1})),
                Identity::Instance("i1".into()),
            )
            .expect("refusal is committed");
        assert_eq!(refused["deadline_refused"], json!(true));
        let phase: String =
            ctl.connection().query_row("SELECT phase FROM instances WHERE id = 'i1'", [], |row| row.get(0)).unwrap();
        assert_eq!(phase, "READY");
        let refused_events: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'goal_deadline_refused'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(refused_events, 1);
        // move the deadline forward: requests begin again
        let future = crate::models::now() + 1000.0;
        ctl.connection().execute("UPDATE goals SET deadline = ?1 WHERE id = 'g1'", [future]).unwrap();
        let ok = ctl
            .submit(
                cmd("b-2", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 1})),
                Identity::Instance("i1".into()),
            )
            .expect("begin after extension");
        assert_eq!(ok["phase"], json!("MODEL_PENDING"));
        ctl.submit(
            cmd(
                "a-1",
                "record_attempt",
                json!({"attempt_id": "at-1", "request_id": "r1", "status": "COMPLETE",
                       "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}),
            ),
            Identity::System,
        )
        .expect("attempt");
        ctl.submit(
            cmd(
                "imp-1",
                "import_response",
                json!({"request_id": "r1", "decision_id": "d-1",
                       "entry": {"role": "assistant", "content": ""},
                       "intents": [{"index": 0, "call_id": "call_0", "name": "shell",
                                    "args": {"command": "echo late"}}]}),
            ),
            Identity::System,
        )
        .expect("import");
        // the deadline passes mid-turn: no new side-effect dispatch either
        ctl.connection()
            .execute("UPDATE goals SET deadline = ?1 WHERE id = 'g1'", [crate::models::now() - 1.0])
            .unwrap();
        let error = ctl
            .submit(
                cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-1:0", "approval_required": false})),
                Identity::System,
            )
            .unwrap_err();
        assert!(error.contains("deadline passed"), "{error}");
        drop(ctl);
        std::fs::remove_file(path).ok();
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
    fn lifecycle_changes_need_the_user_or_a_manage_grant() {
        let (mut ctl, path) = control("lifecycle");
        create_instance(&mut ctl, "i1");
        // an instance without a manage grant cannot pause itself
        let err = ctl
            .submit(
                cmd("sl-0", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "PAUSED"})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("manage grant"), "{err}");
        // delegated management (Q5): the user issues manage@instance:i1 and the
        // holder can pause/resume, but termination still stays with the user
        let issued = ctl
            .submit(
                cmd(
                    "ig-1",
                    "issue_grant",
                    json!({"subject": "i1", "action": "manage", "resource_scope": "instance:i1"}),
                ),
                Identity::User,
            )
            .expect("issue manage");
        assert!(issued["grant_id"].as_str().is_some());
        ctl.submit(cmd("sl-0b", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "ACTIVE"})), Identity::User)
            .expect("reset to active");
        let err = ctl
            .submit(
                cmd("sl-0t", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "TERMINATED"})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("termination"), "{err}");
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
                cmd("dp-a", "dispatch_operation", json!({"operation_id": "d-x:0", "approval_required": true})),
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

    fn revision_of(ctl: &Control, instance: &str) -> i64 {
        ctl.connection()
            .query_row("SELECT revision FROM instances WHERE id = ?1", [instance], |row| row.get(0))
            .unwrap()
    }

    /// Finish import helper: lands the instance in COMPLETION_PENDING with a
    /// stored candidate of the given outcome.
    fn finish_import(ctl: &mut Control, tag: &str, instance: &str, revision: i64, outcome: &str) {
        let request = begin_and_complete(ctl, tag, instance, revision);
        ctl.submit(
            cmd(
                &format!("imp-{tag}"),
                "import_response",
                json!({"request_id": request, "decision_id": format!("d-{tag}"),
                       "entry": {"role": "assistant", "content": ""},
                       "completion": {"outcome": outcome, "summary": "s", "evidence": []}}),
            ),
            Identity::System,
        )
        .expect("import");
    }

    #[test]
    fn create_goal_required_checks_are_user_defined_machine_contracts() {
        let (mut ctl, path) = control("goal-checks-gate");
        create_instance(&mut ctl, "i1");
        // an instance can never mint user-grade machine contracts (§8)
        let err = ctl
            .submit(
                cmd(
                    "g-bad",
                    "create_goal",
                    json!({"id": "g-bad", "limits": {"required_checks": [{"id": "c1", "command": "true"}]}}),
                ),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("required_checks"), "{err}");
        // malformed shapes fail closed
        for limits in [
            json!({"required_checks": [{"id": "c1"}]}),
            json!({"required_checks": [{"id": "c1", "command": "true", "inputs": ["/etc/passwd"]}]}),
            json!({"required_checks": [{"id": "c1", "command": "true", "inputs": ["../escape"]}]}),
            json!({"required_checks": [{"id": "c1", "command": "true", "timeout": 0}]}),
        ] {
            assert!(ctl
                .submit(cmd("g-x", "create_goal", json!({"id": "g-x", "limits": limits})), Identity::User)
                .is_err());
        }
        // the user (or project bootstrap) predefines them; limits persist
        ctl.submit(
            cmd(
                "g-ok",
                "create_goal",
                json!({"id": "g-ok", "limits": {"required_checks": [{"id": "c1", "command": "cargo test",
                                                                        "timeout": 30, "inputs": ["src/lib.rs"]}],
                                                "max_check_rounds": 2}}),
            ),
            Identity::User,
        )
        .expect("goal with checks");
        let limits: String = ctl
            .connection()
            .query_row("SELECT limits_json FROM goals WHERE id = 'g-ok'", [], |row| row.get(0))
            .unwrap();
        let limits: Json = serde_json::from_str(&limits).unwrap();
        assert_eq!(limits["required_checks"][0]["command"], json!("cargo test"));
        assert_eq!(limits["max_check_rounds"], json!(2));
        cleanup(&path);
    }

    #[test]
    fn register_check_runs_commits_the_round_atomically() {
        let (mut ctl, path) = control("check-register");
        create_instance(&mut ctl, "i1");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1"})), Identity::User).expect("goal");
        finish_import(&mut ctl, "fin", "i1", 1, "success");
        assert_eq!(phase_of(&ctl, "i1"), "COMPLETION_PENDING");
        // driver-internal: the user cannot register rounds directly
        assert!(ctl
            .submit(
                cmd(
                    "cr-u",
                    "register_check_runs",
                    json!({"goal_id": "g1", "instance_id": "i1", "round": 1,
                                                         "checks": [{"id": "c1", "command": "true"}]})
                ),
                Identity::User,
            )
            .is_err());
        let registered = ctl
            .submit(
                cmd("cr-1", "register_check_runs", json!({"goal_id": "g1", "instance_id": "i1", "round": 1,
                                                         "checks": [{"id": "c1", "command": "cargo test", "timeout": 30,
                                                                     "inputs": ["src/lib.rs"], "inputs_observed": {"src/lib.rs": "abc"}},
                                                                    {"id": "c2", "command": "cargo clippy"}]})),
                Identity::System,
            )
            .expect("register");
        assert_eq!(registered["operations"], json!(2));
        // synthetic request + decision + two PREPARED ops under the goal
        let request: (String, String, i64) = ctl
            .connection()
            .query_row(
                "SELECT request_ref, status, epoch FROM model_requests WHERE request_id = 'check:g1:1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let epoch: i64 = ctl
            .connection()
            .query_row("SELECT context_epoch FROM instances WHERE id = 'i1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(request, ("required_check".to_string(), "COMPLETE".to_string(), epoch));
        let ops: Vec<(String, String, String)> = ctl
            .connection()
            .prepare("SELECT operation_id, status, intent_json FROM operations WHERE decision_id = 'check:g1:1' ORDER BY tool_index")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().all(|(_, status, _)| status == "PREPARED"));
        let first: Json = serde_json::from_str(&ops[0].2).unwrap();
        assert_eq!(first["name"], json!("shell"));
        assert_eq!(first["args"]["command"], json!("cargo test"));
        assert_eq!(first["args"]["check_id"], json!("c1"));
        assert_eq!(first["args"]["inputs_observed"]["src/lib.rs"], json!("abc"));
        assert_eq!(first["call_id"], json!("check-1-0"));
        // the pairing assistant entry keeps strict wire protocols valid
        let assistant: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i1' AND envelope_id = 'check-round-g1-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let assistant: Json = serde_json::from_str(&assistant).unwrap();
        assert_eq!(assistant["tool_calls"][0]["id"], json!("check-1-0"));
        // replay under a fresh command id is idempotent
        let again = ctl
            .submit(
                cmd(
                    "cr-1b",
                    "register_check_runs",
                    json!({"goal_id": "g1", "instance_id": "i1", "round": 1,
                                                          "checks": [{"id": "c1", "command": "true"}]}),
                ),
                Identity::System,
            )
            .expect("replay");
        assert_eq!(again["already_registered"], json!(true));
        // wrong phase fails closed
        ctl.submit(
            cmd("rs-1", "repair_completion", json!({"instance_id": "i1", "goal_id": "g1", "round": 1})),
            Identity::System,
        )
        .expect("repair");
        assert!(ctl
            .submit(
                cmd(
                    "cr-2",
                    "register_check_runs",
                    json!({"goal_id": "g1", "instance_id": "i1", "round": 2,
                                                         "checks": [{"id": "c1", "command": "true"}]})
                ),
                Identity::System,
            )
            .is_err());
        cleanup(&path);
    }

    #[test]
    fn check_consumption_feeds_context_without_flipping_completion_phase() {
        let (mut ctl, path) = control("check-consume");
        create_instance(&mut ctl, "i1");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1"})), Identity::User).expect("goal");
        finish_import(&mut ctl, "fin", "i1", 1, "success");
        ctl.submit(
            cmd(
                "cr-1",
                "register_check_runs",
                json!({"goal_id": "g1", "instance_id": "i1", "round": 1,
                                                     "checks": [{"id": "c1", "command": "cargo test"}]}),
            ),
            Identity::System,
        )
        .expect("register");
        let before = context_count(&ctl, "i1");
        let completed = ctl
            .submit(
                cmd(
                    "co-1",
                    "complete_operation",
                    json!({"operation_id": "check:g1:1:0", "status": "SUCCEEDED",
                                                         "receipt": {"content": "all tests passed"}}),
                ),
                Identity::System,
            )
            .expect("complete op");
        assert_eq!(completed["decision_open"], json!(false));
        // the receipt lands in context as a tool result for the repair turn,
        // but the completion boundary stays put (§8)
        assert_eq!(context_count(&ctl, "i1"), before + 1);
        assert_eq!(phase_of(&ctl, "i1"), "COMPLETION_PENDING");
        let result: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i1' AND envelope_id = 'check:g1:1:0'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let result: Json = serde_json::from_str(&result).unwrap();
        assert_eq!(result["tool_call_id"], json!("check-1-0"));
        assert!(result["content"].as_str().unwrap().contains("all tests passed"));
        cleanup(&path);
    }

    #[test]
    fn repair_completion_and_block_goal_settle_honestly() {
        let (mut ctl, path) = control("check-settle");
        create_instance(&mut ctl, "i1");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1"})), Identity::User).expect("goal");
        finish_import(&mut ctl, "fin", "i1", 1, "success");
        // repair: back to READY for a repair turn, replay-safe
        assert!(ctl
            .submit(
                cmd("rp-u", "repair_completion", json!({"instance_id": "i1", "goal_id": "g1", "round": 1})),
                Identity::User
            )
            .is_err());
        ctl.submit(
            cmd("rp-1", "repair_completion", json!({"instance_id": "i1", "goal_id": "g1", "round": 1,
                                                    "failures": [{"check_id": "c1", "class": "exit", "reason": "command exited 1"}]})),
            Identity::System,
        )
        .expect("repair");
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        let again = ctl
            .submit(
                cmd("rp-1b", "repair_completion", json!({"instance_id": "i1", "goal_id": "g1", "round": 1})),
                Identity::System,
            )
            .expect("repair replay");
        assert_eq!(again["already_closed"], json!(true));
        // block: settles BLOCKED without upgrading the candidate, idempotent
        let revision = revision_of(&ctl, "i1");
        finish_import(&mut ctl, "fin2", "i1", revision, "success");
        assert!(ctl
            .submit(
                cmd("bg-u", "block_goal", json!({"goal_id": "g1", "instance_id": "i1", "reason": "x"})),
                Identity::User
            )
            .is_err());
        let blocked = ctl
            .submit(
                cmd(
                    "bg-1",
                    "block_goal",
                    json!({"goal_id": "g1", "instance_id": "i1",
                                                     "reason": "required checks failed (c1:exit) after 3 round(s)"}),
                ),
                Identity::System,
            )
            .expect("block");
        assert_eq!(blocked["status"], json!("BLOCKED"));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        let status: String =
            ctl.connection().query_row("SELECT status FROM goals WHERE id = 'g1'", [], |row| row.get(0)).unwrap();
        assert_eq!(status, "BLOCKED");
        let again = ctl
            .submit(
                cmd("bg-1b", "block_goal", json!({"goal_id": "g1", "instance_id": "i1", "reason": "x"})),
                Identity::System,
            )
            .expect("block replay");
        assert_eq!(again["already_closed"], json!(true));
        let events: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'goal_blocked'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(events, 1);
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
