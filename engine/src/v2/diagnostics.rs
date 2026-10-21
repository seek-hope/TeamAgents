//! D-461: read-only work diagnostics. Call inside the caller's read transaction.
//! These are observations, not a scheduler or a deadlock verdict. In particular,
//! silence, age and a wait without a timer never authorize another model request.

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;

pub(super) struct View {
    pub activities: BTreeMap<String, Json>,
    pub tasks: Vec<Json>,
}

fn diagnosis(code: &str, summary: &str, attention: bool, next_steps: &str) -> Json {
    json!({"code": code, "summary": summary, "attention": attention, "next_steps": next_steps,
        "reason": null, "waits": [], "operations": []})
}

/// One derivation shared by checkpoint, instances and tasks. No private message
/// text is needed to distinguish runnable input from an addressed assistant tail.
/// ponytail: per-instance probes suit local teams; batch these queries if
/// large-team checkpoint measurements justify it, without changing the snapshot.
pub(super) fn read(conn: &Connection, include_settled: bool) -> Result<View, String> {
    read_sql(conn, include_settled).map_err(|e| format!("work diagnostics: {e}"))
}

fn read_sql(conn: &Connection, include_settled: bool) -> rusqlite::Result<View> {
    let mut activities = BTreeMap::new();
    let mut stmt =
        conn.prepare("SELECT id, lifecycle, phase, context_epoch, active_request_id FROM instances ORDER BY id")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let lifecycle: String = row.get(1)?;
        let phase: String = row.get(2)?;
        let epoch: i64 = row.get(3)?;
        let request: Option<String> = row.get(4)?;
        let mut waits_stmt = conn.prepare(
            "SELECT id, mode, conditions_json, timer_at FROM waits
             WHERE instance_id = ?1 AND epoch = ?2 AND status = 'PENDING' ORDER BY rowid",
        )?;
        let waits = waits_stmt
            .query_map(rusqlite::params![id, epoch], |r| {
                let conditions: String = r.get(2)?;
                Ok(json!({"id": r.get::<_, String>(0)?, "mode": r.get::<_, String>(1)?,
                "conditions": serde_json::from_str::<Json>(&conditions).unwrap_or(Json::Null),
                "timer_at": r.get::<_, Option<f64>>(3)?}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut operations_stmt = conn.prepare(
            "SELECT o.operation_id, o.status, o.cancel_requested FROM operations o
             JOIN decisions d ON d.decision_id = o.decision_id
             WHERE d.request_id = ?1 AND o.epoch = ?2 ORDER BY o.tool_index",
        )?;
        let operations = operations_stmt
            .query_map(rusqlite::params![request, epoch], |r| {
                Ok(json!({"id": r.get::<_, String>(0)?, "status": r.get::<_, String>(1)?,
                "cancel_requested": r.get::<_, i64>(2)? != 0}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let approval: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM approvals a JOIN operations o ON o.operation_id = a.operation_id
             JOIN decisions d ON d.decision_id = o.decision_id WHERE d.request_id = ?1 AND o.epoch = ?2
             AND a.status = 'PENDING' AND o.status IN ('PREPARED', 'DISPATCH_COMMITTED', 'RUNNING'))",
            rusqlite::params![request, epoch],
            |r| r.get(0),
        )?;
        let incoming: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM envelopes WHERE recipient = ?1 AND epoch = ?2 AND state = 'ACCEPTED')",
            rusqlite::params![id, epoch],
            |r| r.get(0),
        )?;
        let (pending, running): (i64, i64) = conn.query_row(
            "SELECT COUNT(CASE WHEN status = 'PENDING' THEN 1 END),
                    COUNT(CASE WHEN status = 'RUNNING' THEN 1 END) FROM tasks WHERE assignee = ?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        // Mirror the visible ordering in driver::read_entries, not the raw last
        // row: a compression summary precedes its retained tail even at a higher idx.
        // Use the primary-key tail index rather than sorting the entire visible
        // history on every checkpoint. Only a summary-only view needs a second probe.
        let mut tail: Option<String> = conn
            .query_row(
                "SELECT kind FROM context_entries WHERE instance_id = ?1 AND epoch = ?2
                 AND compressed_by IS NULL AND kind != 'summary' ORDER BY idx DESC LIMIT 1",
                rusqlite::params![id, epoch],
                |r| r.get(0),
            )
            .optional()?;
        if tail.is_none() {
            tail = conn
                .query_row(
                    "SELECT kind FROM context_entries WHERE instance_id = ?1 AND epoch = ?2
                 AND compressed_by IS NULL AND kind = 'summary' ORDER BY idx DESC LIMIT 1",
                    rusqlite::params![id, epoch],
                    |r| r.get(0),
                )
                .optional()?;
        }
        let runnable =
            incoming || pending > 0 || tail.as_deref().is_some_and(|kind| !matches!(kind, "assistant" | "runtime"));
        let mut activity = match (lifecycle.as_str(), phase.as_str()) {
            ("TERMINATED", _) => diagnosis("terminated", "Instance terminated; it cannot resume.", false,
                "Assign any remaining work to a new instance; inspect receipts before retrying effects."),
            ("PAUSED" | "PARKED", _) => diagnosis("inactive", "Instance paused or parked; inspect its lifecycle reason.", true,
                "Fix the cause, then instances resume --id INSTANCE. In-flight tools may still run; inspect operations. Unknown effects must be verified before retrying."),
            ("ACTIVE", "WAITING_CAPABILITY") => diagnosis("waiting_capability", "Waiting for capability approval.", true,
                "Use authority requests, then authority approve or authority deny --id REQUEST. No capability is granted by this diagnosis."),
            ("ACTIVE", _) if approval => diagnosis("waiting_approval", "Waiting for operation approval.", true,
                "Use approvals, then approvals approve or approvals deny --id APPROVAL. This approves one operation, not a capability."),
            ("ACTIVE", "MODEL_PENDING") => diagnosis("model_pending", "Model request in flight; duration alone does not imply a stall.", false,
                "Wait, or instances interrupt --id INSTANCE to interrupt only the model request."),
            ("ACTIVE", "TOOLS_PENDING") => diagnosis("tools_pending", "Processing tool operations; duration alone does not imply a stall.", false,
                "Inspect operations and history. To stop running work use instances terminate --id INSTANCE --yes; tasks cancel and model interrupt do not stop tools."),
            ("ACTIVE", "COMPLETION_PENDING") => diagnosis("checking", "Processing completion checks or settlement.", false,
                "Inspect history for check results; do not infer success from this phase."),
            ("ACTIVE", "WAITING") if waits.is_empty() => diagnosis("wait_missing", "WAITING phase has no pending wait in the current epoch.", true,
                "Inspect status and history; this is an inconsistent snapshot, not permission to replay work."),
            ("ACTIVE", "WAITING") => {
                let timed = waits.iter().any(|wait| !wait["timer_at"].is_null());
                diagnosis("waiting", if timed {
                    "Waiting on registered conditions; a timer is recorded."
                } else {
                    "Waiting on registered conditions; no timer is recorded (not proof of deadlock)."
                }, false, "Inspect waits and tasks. BLOCKED and idle assignees do not satisfy task waits. Cancel a task only if its result is no longer needed; no automatic wakeup is added.")
            }
            ("ACTIVE", "READY") if runnable => diagnosis("ready", "Unaddressed input or a pending task is ready for processing.", false,
                "Allow the coordinator to process it; this snapshot does not prove a model request has started."),
            ("ACTIVE", "READY") if running > 0 => diagnosis("idle_unsettled", "Assignee idle with RUNNING task(s) not settled.", true,
                "Send an explicit instruction with instances send --id INSTANCE --prompt TEXT, or tasks cancel --id TASK if the result is no longer needed. No automatic retry occurs; task cancellation does not stop tools."),
            ("ACTIVE", "READY") => diagnosis("idle", "No unaddressed work is visible at this snapshot.", false, "Send a new instruction when needed."),
            _ => diagnosis("unknown", "Unrecognized lifecycle or phase; no progress inference made.", true, "Inspect status and history."),
        };
        if matches!(lifecycle.as_str(), "PAUSED" | "PARKED") {
            let reason: Option<String> = conn
                .query_row(
                    "SELECT substr(json_extract(payload_json, '$.reason'), 1, 512) FROM events
                 WHERE kind = 'instance_lifecycle' AND scope = ?1
                 AND json_type(payload_json, '$.reason') = 'text' ORDER BY sequence DESC LIMIT 1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            activity["reason"] = json!(reason);
        }
        activity["waits"] = json!(waits);
        activity["operations"] = json!(operations);
        activities.insert(id, activity);
    }
    let mut stmt = conn.prepare(
        "SELECT id, goal_id, assignee, status FROM tasks
         WHERE ?1 OR status NOT IN ('SUCCEEDED', 'FAILED', 'CANCELLED') ORDER BY rowid",
    )?;
    let tasks = stmt.query_map([include_settled], |row| {
        let id: String = row.get(0)?;
        let assignee: String = row.get(2)?;
        let status: String = row.get(3)?;
        let activity = activities.get(&assignee).cloned().unwrap_or_else(|| diagnosis(
            "missing_assignee", "Task assignee is missing.", true, "Inspect history; cancel obsolete work explicitly.",
        ));
        let mut diagnostic = match status.as_str() {
            "SUCCEEDED" | "FAILED" | "CANCELLED" => diagnosis("settled", "Task settled; its assignee may still have running work.", false, "Inspect assignee activity before assuming any command stopped."),
            "BLOCKED" => diagnosis("blocked", "Task BLOCKED; it does not satisfy a task-result wait.", true,
                "Inspect history and assignee activity. Verify unknown effects first; cancel obsolete work with tasks cancel --id TASK and delegate replacement work explicitly. Cancellation does not stop commands."),
            _ => activity.clone(),
        };
        if status == "BLOCKED" {
            // Runtime block reasons are durable. A model's BLOCKED settlement
            // need not have this event: null means unavailable, never guessed.
            let reason: Option<String> = conn.query_row(
                "SELECT substr(json_extract(payload_json, '$.reason'), 1, 512) FROM events
                 WHERE kind = 'task_blocked' AND scope = ?1 ORDER BY sequence DESC LIMIT 1",
                [&id], |r| r.get(0),
            ).optional()?.flatten();
            diagnostic["reason"] = json!(reason);
        }
        Ok(json!({"id": id, "goal_id": row.get::<_, String>(1)?, "assignee": assignee,
            "status": status, "diagnosis": diagnostic, "assignee_activity": activity}))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(View { activities, tasks })
}

#[cfg(test)]
mod tests;
