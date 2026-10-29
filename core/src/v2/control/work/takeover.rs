//! A cancelled assignment can still own a running host command. Stop observations are
//! separate from immutable receipts and never turn an unknown outcome into success.

use super::*;

fn read_only(name: &str) -> bool {
    matches!(
        name,
        "read_file" | "read_artifact" | "read_history" | "grep" | "glob" | "skill" | "web_search" | "web_fetch"
    ) || [
        crate::kernel::SEND_TOOL,
        crate::kernel::SPAWN_TOOL,
        crate::kernel::DELEGATE_TOOL,
        crate::kernel::CANCEL_TOOL,
        crate::kernel::VERIFY_TOOL,
    ]
    .contains(&name)
}

fn retired(tx: &Connection, request: &str) -> Result<bool, String> {
    let (instance,epoch,goal,current_epoch,lifecycle,context,request_ref):(String,i64,Option<String>,i64,String,Option<String>,String)=tx.query_row(
        "SELECT r.instance_id,r.epoch,r.goal_id,i.context_epoch,i.lifecycle,w.context_json,r.request_ref FROM model_requests r
        JOIN instances i ON i.id=r.instance_id LEFT JOIN work_requests w ON w.request_id=r.request_id WHERE r.request_id=?1",
        [request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(|e|format!("execution scope: {e}"))?;
    if epoch != current_epoch || lifecycle != "ACTIVE" {
        return Ok(true);
    }
    if request_ref == "required_check" {
        return tx
            .query_row("SELECT NOT EXISTS(SELECT 1 FROM goals WHERE id=?1 AND status='ACTIVE')", [goal], |r| r.get(0))
            .map_err(|e| format!("check execution scope: {e}"));
    }
    if goal != budget_goal(tx, &instance)? {
        return Ok(true);
    }
    if let Some(context) = context {
        let context: Json = serde_json::from_str(&context).map_err(|e| format!("execution work scope: {e}"))?;
        if context["admission"] == true {
            return Ok(true);
        }
        if let Some(task) = context["task_id"].as_str().filter(|task| !task.is_empty()) {
            let active: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND status IN ('RUNNING','PENDING'))",
                    [task],
                    |r| r.get(0),
                )
                .map_err(|e| format!("execution assignment: {e}"))?;
            return Ok(!active);
        }
        return Ok(false);
    }
    // ponytail: legacy requests have no exact task attribution. A failed/cancelled
    // assignment is conservatively ambiguous until its tracked execution stops.
    tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE assignee=?1 AND goal_id=?2 AND status IN ('FAILED','BLOCKED','CANCELLED'))",
        rusqlite::params![instance,goal],|r|r.get(0)).map_err(|e|format!("legacy execution scope: {e}"))
}

pub(in crate::v2::control) fn dispatch_scope(tx: &Connection, operation: &str) -> Result<(), String> {
    if policy(tx)?.is_none() {
        return Ok(());
    }
    let request:String=tx.query_row("SELECT d.request_id FROM operations o JOIN decisions d ON d.decision_id=o.decision_id WHERE o.operation_id=?1",
        [operation],|r|r.get(0)).map_err(|e|format!("operation scope: {e}"))?;
    if retired(tx, &request)? {
        return Err("The operation belongs to stopped or superseded work; no new dispatch is permitted".into());
    }
    Ok(())
}

pub(in crate::v2::control) fn candidates(tx: &Connection, instance: &str) -> Result<Vec<Json>, String> {
    if policy(tx)?.is_none() {
        return Ok(Vec::new());
    }
    let workspace: String = tx
        .query_row("SELECT workspace_ref FROM instances WHERE id=?1", [instance], |r| r.get(0))
        .map_err(|e| format!("takeover workspace: {e}"))?;
    let mut stmt=tx.prepare("SELECT o.operation_id,r.request_id,r.instance_id,i.workspace_ref,o.status,json_extract(o.intent_json,'$.name')
        FROM operations o JOIN decisions d ON d.decision_id=o.decision_id JOIN model_requests r ON r.request_id=d.request_id
        JOIN instances i ON i.id=r.instance_id LEFT JOIN work_requests w ON w.request_id=r.request_id
        LEFT JOIN goals g ON g.id=r.goal_id LEFT JOIN tasks t ON t.id=json_extract(w.context_json,'$.task_id')
        WHERE o.quiescence_json IS NULL
        AND (i.lifecycle<>'ACTIVE' OR r.epoch<>i.context_epoch OR g.status<>'ACTIVE'
            OR t.status NOT IN ('RUNNING','PENDING')
            OR (COALESCE(json_extract(w.context_json,'$.task_id'),'')='' AND i.active_goal_id IS NOT r.goal_id)
            OR (w.request_id IS NULL AND EXISTS(SELECT 1 FROM tasks legacy WHERE legacy.assignee=r.instance_id
                AND legacy.goal_id=r.goal_id AND legacy.status IN ('FAILED','BLOCKED','CANCELLED'))))
        AND o.status NOT IN ('PREPARED','CANCELLED_BEFORE_START')
        AND NOT (o.status IN ('SUCCEEDED','FAILED','CANCELLED') AND COALESCE(json_extract(o.receipt_json,'$.started'),1)=0)
        AND ((json_extract(o.intent_json,'$.name')='shell' AND COALESCE(t.status,g.status,'SUCCEEDED')<>'SUCCEEDED')
            OR o.status IN ('DISPATCH_COMMITTED','RUNNING','OUTCOME_UNKNOWN')) ORDER BY o.rowid")
        .map_err(|e|format!("takeover executions: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| format!("takeover execution query: {e}"))?;
    let mut candidates = Vec::new();
    for row in rows {
        let (old, request, owner, path, status, name) = row.map_err(|e| format!("takeover execution row: {e}"))?;
        if read_only(&name) {
            continue;
        }
        let overlap =
            std::path::Path::new(&workspace).starts_with(&path) || std::path::Path::new(&path).starts_with(&workspace);
        if overlap && retired(tx, &request)? {
            candidates.push(json!({"operation_id":old,"instance_id":owner,"status":status,"tool":name,
                "reason":format!("Shared workspace takeover is blocked by {old} on {owner} ({status}). Cancellation does not prove execution stopped. Wait for recorded stop evidence, ask the user to stop it, or use an independent isolated workspace and integrate explicitly.")}));
            if candidates.len() >= 64 {
                break;
            }
        }
    }
    Ok(candidates)
}

pub(in crate::v2::control) fn gate(
    tx: &Connection,
    session: &str,
    operation: &str,
    instance: &str,
    intent: &Json,
) -> Result<Option<Json>, String> {
    if read_only(intent["name"].as_str().unwrap_or("")) {
        return Ok(None);
    }
    let blocked = candidates(tx, instance)?.into_iter().next();
    if let Some(blocked) = blocked {
        tx.execute(
            "UPDATE operations SET dispatch_wait_json=?2 WHERE operation_id=?1",
            rusqlite::params![operation, blocked.to_string()],
        )
        .map_err(|e| format!("record takeover blocker: {e}"))?;
        event(tx, session, "workspace_takeover_blocked", operation, &blocked)?;
        return Ok(Some(json!({"operation_id":operation,"status":"WORKSPACE_BUSY","blocker":blocked})));
    }
    Ok(None)
}

pub(in crate::v2::control) fn observe_many(
    tx: &Connection,
    session: &str,
    params: &Json,
    identity: &Identity,
) -> Result<Json, String> {
    let observations = params["observations"]
        .as_array()
        .filter(|items| items.len() <= 64)
        .ok_or("at most 64 stop observations are allowed")?;
    let mut results = Vec::new();
    for observation in observations {
        results.push(stopped(tx, session, observation, identity)?);
    }
    Ok(json!({"observations":results}))
}

fn stopped(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::System) {
        return Err("only the runtime records observed process stops".into());
    }
    let operation = params["operation_id"].as_str().ok_or("observe_operation_stop.operation_id required")?;
    let observation = &params["observation"];
    if observation["source"] != "runner_process_group"
        || !observation.is_object()
        || observation.to_string().len() > 4096
    {
        return Err("a bounded runner process-group observation is required".into());
    }
    let changed=tx.execute("UPDATE operations SET quiescence_json=?2 WHERE operation_id=?1 AND quiescence_json IS NULL AND json_extract(intent_json,'$.name')='shell'",
        rusqlite::params![operation,observation.to_string()]).map_err(|e|format!("record execution stop: {e}"))?;
    if changed > 0 {
        let mut stmt = tx
            .prepare(
                "SELECT r.instance_id,w.context_json FROM operations o JOIN decisions d ON d.decision_id=o.decision_id
            JOIN model_requests r ON r.request_id=d.request_id JOIN work_requests w ON w.request_id=r.request_id
            WHERE json_extract(o.dispatch_wait_json,'$.operation_id')=?1",
            )
            .map_err(|e| format!("workspace waiters: {e}"))?;
        let waiters = stmt
            .query_map([operation], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| format!("workspace waiters query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("workspace waiters collect: {e}"))?;
        let mut notified = std::collections::BTreeSet::new();
        for (instance, context) in waiters {
            let old: Json = serde_json::from_str(&context).map_err(|e| format!("workspace waiter scope: {e}"))?;
            let current = view(tx, &instance)?;
            if current["goal_id"] == old["goal_id"]
                && current["epoch"] == old["epoch"]
                && current["task_id"] == old["task_id"]
            {
                if let Some(key) = key(tx, &instance)? {
                    if !notified.insert(key.clone()) {
                        continue;
                    }
                    let scope = crate::kernel::args_hash(&json!(key));
                    queue_envelope(
                        tx,
                        session,
                        &Outbox {
                            sender: "runtime",
                            recipient: &instance,
                            epoch: key.2,
                            kind: "workspace_available",
                            correlation_id: Some(&scope),
                            payload: &json!({"operation_id":operation,"reason":"The previously conflicting process group has stopped. Its recorded outcome is unchanged; inspect receipts before resuming work.","provenance":"runtime"}),
                        },
                    )?;
                    wake_attention(tx, session, &instance)?;
                }
            }
        }
        event(tx, session, "operation_stop_observed", operation, observation)?;
    }
    Ok(json!({"operation_id":operation,"recorded":changed>0}))
}
