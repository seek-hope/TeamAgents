//! D-487: durable work decisions. Every mutation runs inside Control::submit's transaction.
//! Model checkpoints are claims; grants, checks, usage and original input remain authoritative elsewhere.

use super::*;

mod admission;
pub(super) mod takeover;

pub(super) fn admission_request(tx: &Connection, request: &str) -> Result<bool, String> {
    Ok(admission::request(tx, request)?.is_some())
}

pub(super) fn admission_budget(
    tx: &Connection,
    session: &str,
    instance: &str,
    estimate: i64,
) -> Result<Option<String>, String> {
    let reason = admission::budget(tx, instance, estimate)?;
    if let Some(reason) = &reason {
        set_lifecycle(
            tx,
            session,
            &json!({"instance_id":instance,"lifecycle":"PARKED","reason":reason}),
            &Identity::System,
        )?;
        event(tx, session, "work_admission_refused", instance, &json!({"reason":reason}))?;
    }
    Ok(reason)
}

pub(super) fn freeze_admission(tx: &Connection, instance: &str, request: &str) -> Result<(), String> {
    let context = admission::view(tx, instance)?;
    if !context.is_null() {
        tx.execute(
            "INSERT INTO work_requests(request_id,context_json) VALUES (?1,?2)",
            rusqlite::params![request, context.to_string()],
        )
        .map_err(|e| format!("freeze admission context: {e}"))?;
    }
    Ok(())
}

fn current_key(tx: &Connection, instance: &str) -> Result<Option<WorkKey>, String> {
    if let Some(key) = key(tx, instance)? {
        return Ok(Some(key));
    }
    let pending = admission::view(tx, instance)?;
    Ok((!pending.is_null())
        .then(|| (String::new(), instance.into(), pending["epoch"].as_i64().unwrap_or(0), String::new())))
}

const POLICY_KEY: &str = "work_policy_v1";
const REPAIR_LIMIT: i64 = 2;
const CHECKPOINT_BYTES: usize = 32 * 1024;

type WorkKey = (String, String, i64, String);

pub(super) fn policy(tx: &Connection) -> Result<Option<Json>, String> {
    let value: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key = ?1", [POLICY_KEY], |r| r.get(0))
        .optional()
        .map_err(|e| format!("work policy: {e}"))?;
    value.map(|s| serde_json::from_str(&s).map_err(|e| format!("work policy JSON: {e}"))).transpose()
}

pub(super) fn configure(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    if !matches!(identity, Identity::User | Identity::System) {
        return Err("only the user or session bootstrap configures work policy".into());
    }
    let leader = params["leader_id"].as_str().ok_or("configure_work.leader_id required")?;
    if load_instance(tx, leader)?.0 != session {
        return Err("work leader belongs to another session".into());
    }
    let limits = params.get("limits").cloned().unwrap_or(json!({}));
    if !limits.is_object() {
        return Err("configure_work.limits must be an object".into());
    }
    if let Some(checks) = limits.get("required_checks") {
        validate_required_checks(checks)?;
    }
    for field in ["max_total_tokens", "deadline_minutes"] {
        if limits.get(field).is_some_and(|value| value.as_u64().is_none()) {
            return Err(format!("configure_work.limits.{field} must be a non-negative integer"));
        }
    }
    let value = json!({"version": 1, "leader_id": leader, "limits": limits});
    tx.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![POLICY_KEY, value.to_string()],
    )
    .map_err(|e| format!("configure work: {e}"))?;
    // Configuration alone never enrolls old idle work or resumes an inactive instance.
    event(tx, session, "work_policy_configured", leader, &json!({"version": 1}))?;
    Ok(json!({"version": 1}))
}

fn key(tx: &Connection, instance: &str) -> Result<Option<WorkKey>, String> {
    let Some(goal) = budget_goal(tx, instance)? else { return Ok(None) };
    let (_, epoch, _, _, _) = load_instance(tx, instance)?;
    let owned: Option<String> = tx
        .query_row("SELECT active_goal_id FROM instances WHERE id=?1", [instance], |r| r.get(0))
        .map_err(|e| format!("work owner: {e}"))?;
    let task: String = if owned.as_deref() == Some(&goal) {
        String::new()
    } else {
        tx.query_row(
            "SELECT id FROM tasks WHERE assignee=?1 AND goal_id=?2 AND status IN ('RUNNING','PENDING')
            ORDER BY CASE status WHEN 'RUNNING' THEN 0 ELSE 1 END, rowid LIMIT 1",
            rusqlite::params![instance, goal],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("work task: {e}"))?
        .unwrap_or_default()
    };
    Ok(Some((goal, instance.into(), epoch, task)))
}

fn ensure(tx: &Connection, k: &WorkKey) -> Result<(), String> {
    tx.execute(
        "INSERT OR IGNORE INTO work_states(goal_id, instance_id, epoch, task_id) VALUES (?1,?2,?3,?4)",
        rusqlite::params![k.0, k.1, k.2, k.3],
    )
    .map_err(|e| format!("enroll work: {e}"))?;
    Ok(())
}

pub(super) fn view(tx: &Connection, instance: &str) -> Result<Json, String> {
    if key(tx, instance)?.is_none() {
        let pending = admission::view(tx, instance)?;
        if !pending.is_null() {
            return Ok(pending);
        }
    }
    let (k, closed_state) = match key(tx, instance)? {
        Some(key) => (key, None),
        None => {
            let closed: Option<(WorkKey, String)> = tx
                .query_row(
                    "SELECT w.goal_id,w.instance_id,w.epoch,w.task_id,COALESCE(t.status,g.status)
                FROM work_states w JOIN goals g ON g.id=w.goal_id LEFT JOIN tasks t ON t.id=w.task_id
                WHERE w.instance_id=?1
                    AND ((w.task_id='' AND g.status<>'ACTIVE') OR t.status NOT IN ('PENDING','RUNNING'))
                ORDER BY w.rowid DESC LIMIT 1",
                    [instance],
                    |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?), r.get(4)?)),
                )
                .optional()
                .map_err(|e| format!("closed work view: {e}"))?;
            let Some((key, status)) = closed else { return Ok(Json::Null) };
            (
                key,
                Some(match status.as_str() {
                    "BLOCKED" => "BLOCKED",
                    "CANCELLED" => "STOPPED",
                    _ => "COMPLETE",
                }),
            )
        }
    };
    tx.query_row("SELECT state, checkpoint_json, pending_reason, empty_turns, revision, reason,
        COALESCE(json_extract(policy_json,'$.inherited_execution'),0)
        FROM work_states WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
        rusqlite::params![k.0,k.1,k.2,k.3], |r| {
            let checkpoint: String = r.get(1)?;
            let state:String=r.get(0)?;
            Ok(json!({"policy_version":1, "goal_id":k.0, "instance_id":k.1, "epoch":k.2, "task_id":k.3,
                "state":closed_state.unwrap_or(&state), "checkpoint":serde_json::from_str::<Json>(&checkpoint).unwrap_or(Json::Null),
                "pending_reason":r.get::<_,String>(2)?, "repair_turns":r.get::<_,i64>(3)?,
                "revision":r.get::<_,i64>(4)?, "reason":r.get::<_,String>(5)?, "needs_fresh_evidence":r.get::<_,bool>(6)?, "checkpoint_provenance":"model_claim"}))
        }).optional().map(|r| r.unwrap_or(Json::Null)).map_err(|e| format!("work view: {e}"))
}

pub(super) fn check_prepared(tx: &Connection, instance: &str, prepared: &Json) -> Result<(), String> {
    let current = view(tx, instance)?;
    for field in ["goal_id", "epoch", "task_id", "admission_id", "revision", "state"] {
        if current[field] != prepared[field] {
            return Err("stale executor: work changed during request preparation".into());
        }
    }
    Ok(())
}

pub(super) fn context(tx: &Connection, instance: &str) -> Result<Json, String> {
    let mut result = view(tx, instance)?;
    if result.is_null() || result["admission"] == true {
        return Ok(result);
    }
    let mut stmt = tx
        .prepare(
            "SELECT c.id, c.message_json FROM work_inputs w JOIN context_entries c
        ON c.instance_id=w.instance_id AND c.epoch=w.epoch AND c.envelope_id=w.envelope_id
        WHERE w.goal_id=?1 AND w.instance_id=?2 ORDER BY w.rowid DESC LIMIT 16",
        )
        .map_err(|e| format!("work input context: {e}"))?;
    let inputs = stmt
        .query_map(rusqlite::params![result["goal_id"].as_str(), instance], |r| {
            let raw: String = r.get(1)?;
            let message: Json = serde_json::from_str(&raw).unwrap_or(Json::Null);
            let text = message["content"].as_str().unwrap_or("");
            Ok(json!({"context_ref":r.get::<_,String>(0)?, "text":text.chars().take(1500).collect::<String>(),
            "truncated":text.chars().count()>1500, "provenance":"user_input"}))
        })
        .map_err(|e| format!("work inputs query: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("work inputs collect: {e}"))?;
    result["recent_user_inputs"] = json!(inputs.into_iter().rev().collect::<Vec<_>>());
    result["original_input"] = tx
        .query_row(
            "SELECT c.id,c.message_json FROM goals g JOIN context_entries c
        ON c.envelope_id=g.original_request_ref AND c.instance_id=?2 WHERE g.id=?1 ORDER BY c.rowid LIMIT 1",
            rusqlite::params![result["goal_id"].as_str(), instance],
            |r| {
                let raw: String = r.get(1)?;
                let message: Json = serde_json::from_str(&raw).unwrap_or(Json::Null);
                let text = message["content"].as_str().unwrap_or("");
                Ok(json!({"context_ref":r.get::<_,String>(0)?,"text":text.chars().take(1500).collect::<String>(),
                "truncated":text.chars().count()>1500,"provenance":"user_input"}))
            },
        )
        .optional()
        .map_err(|e| format!("original work input: {e}"))?
        .unwrap_or(Json::Null);
    let mut operations=tx.prepare("SELECT o.operation_id,o.status,json_extract(o.intent_json,'$.call_id'),json_extract(o.intent_json,'$.name')
        FROM operations o JOIN decisions d ON d.decision_id=o.decision_id JOIN model_requests r ON r.request_id=d.request_id
        LEFT JOIN work_requests w ON w.request_id=r.request_id
        WHERE r.instance_id=?1 AND r.goal_id=?2 AND (?3='' OR json_extract(w.context_json,'$.task_id')=?3)
        ORDER BY o.rowid DESC LIMIT 17").map_err(|e|format!("work observations: {e}"))?;
    let mut observed = operations
        .query_map(rusqlite::params![instance, result["goal_id"].as_str(), result["task_id"].as_str()], |r| {
            Ok(json!({"operation_id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,
            "call_id":r.get::<_,Option<String>>(2)?,"tool":r.get::<_,Option<String>>(3)?}))
        })
        .map_err(|e| format!("work observations query: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("work observations collect: {e}"))?;
    let truncated = observed.len() > 16;
    observed.truncate(16);
    result["observations"] = json!({"provenance":"runtime","operations":observed,"truncated":truncated,
        "meaning":"Recorded execution outcomes; a successful operation alone does not prove the whole user request is complete."});
    Ok(result)
}

pub(super) fn reset_epoch(tx: &Connection, instance: &str, old: i64, new: i64) -> Result<(), String> {
    tx.execute("INSERT INTO work_states(goal_id,instance_id,epoch,task_id,state,checkpoint_json,policy_json,pending_reason,
        empty_turns,revision,reason,fingerprints_json)
        SELECT w.goal_id,w.instance_id,?3,w.task_id,w.state,w.checkpoint_json,w.policy_json,
            CASE WHEN w.state='RUNNING' THEN 'Context reset; inspect saved work and receipts before continuing' ELSE w.pending_reason END,
            w.empty_turns,w.revision+1,w.reason,w.fingerprints_json
        FROM work_states w LEFT JOIN goals g ON g.id=w.goal_id
        WHERE w.instance_id=?1 AND w.epoch=?2 AND (g.status='ACTIVE' OR w.goal_id='')",rusqlite::params![instance,old,new])
        .map_err(|e|format!("preserve work across context reset: {e}"))?;
    Ok(())
}

pub(super) fn admit_input(tx: &Connection, session: &str, instance: &str, envelope: &str) -> Result<(), String> {
    let Some(settings) = policy(tx)? else { return Ok(()) };
    let (_, epoch, lifecycle, _, _) = load_instance(tx, instance)?;
    if budget_goal(tx, instance)?.is_none() && settings["leader_id"].as_str() == Some(instance) {
        // A new admitted input gets a new record; it never reopens a terminal goal.
        let latest: Option<String> = tx
            .query_row("SELECT status FROM goals WHERE session_id=?1 ORDER BY rowid DESC LIMIT 1", [session], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| format!("last work goal: {e}"))?;
        if matches!(latest.as_deref(), Some("BLOCKED" | "CANCELLED")) {
            return Err(
                "the previous goal is blocked or cancelled; explicitly create a new goal before admitting more work"
                    .into(),
            );
        }
        return admission::queue(tx, session, instance, epoch, envelope, &settings);
    }
    let Some(k) = key(tx, instance)? else { return Ok(()) };
    ensure(tx, &k)?;
    let payload: String = tx
        .query_row("SELECT payload_json FROM envelopes WHERE id=?1", [envelope], |r| r.get(0))
        .map_err(|e| format!("work input policy: {e}"))?;
    let payload: Json = serde_json::from_str(&payload).map_err(|e| format!("work input JSON: {e}"))?;
    if let Some(checks) = payload.get("checks").filter(|v| v.as_array().is_some_and(|a| !a.is_empty())) {
        require_checks(tx, session, &json!({"goal_id":k.0,"checks":checks}), &Identity::User)?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO work_inputs(goal_id,instance_id,epoch,envelope_id)
            SELECT ?1,?2,?3,?4 WHERE NOT EXISTS(SELECT 1 FROM work_inputs WHERE instance_id=?2 AND envelope_id=?4)",
        rusqlite::params![k.0, instance, epoch, envelope],
    )
    .map_err(|e| format!("work input: {e}"))?;
    tx.execute(
        "UPDATE goals SET original_request_ref=?1 WHERE id=?2 AND original_request_ref=''",
        rusqlite::params![envelope, k.0],
    )
    .map_err(|e| format!("work original input: {e}"))?;
    if lifecycle == "ACTIVE" {
        reset(tx, &k, "User input admitted")?;
    } else {
        set_state(tx, &k, "STOPPED", "Input recorded; the instance requires explicit user recovery", "")?;
    }
    event(
        tx,
        session,
        "work_input_admitted",
        instance,
        &json!({"goal_id":k.0,"envelope_id":envelope,"policy_version":1}),
    )?;
    Ok(())
}

pub(super) fn assigned(tx: &Connection, instance: &str) -> Result<(), String> {
    let Some(k) = key(tx, instance)? else { return Ok(()) };
    let enrolled: bool = tx
        .query_row("SELECT EXISTS(SELECT 1 FROM work_states WHERE goal_id=?1)", [&k.0], |r| r.get(0))
        .map_err(|e| format!("parent work enrollment: {e}"))?;
    if enrolled {
        ensure(tx, &k)?;
    }
    Ok(())
}

fn reset(tx: &Connection, k: &WorkKey, reason: &str) -> Result<(), String> {
    tx.execute(
        "UPDATE work_states SET state='RUNNING', pending_reason=?5, empty_turns=0, fingerprints_json='[]',
        reason='', revision=revision+1 WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
        rusqlite::params![k.0, k.1, k.2, k.3, reason],
    )
    .map_err(|e| format!("reset work episode: {e}"))?;
    Ok(())
}

pub(super) fn observation(
    tx: &Connection,
    instance: &str,
    kind: &str,
    sender: &str,
    correlation: Option<&str>,
) -> Result<(), String> {
    if view(tx, instance)?["state"] == "STOPPED" {
        return Ok(());
    }
    let Some(k) = key(tx, instance)? else { return Ok(()) };
    if kind == "workspace_available" && sender == "runtime" {
        if correlation == Some(crate::kernel::args_hash(&json!(k)).as_str())
            && view(tx, instance)?["state"] != "NEEDS_INPUT"
        {
            reset(
                tx,
                &k,
                "A conflicting process group has stopped; inspect its unchanged receipt before resuming work",
            )?;
        }
    } else if matches!(kind, "task_result" | "task_attention") {
        let matches_goal = correlation
            .and_then(|task| {
                tx.query_row("SELECT goal_id FROM tasks WHERE id=?1", [task], |r| r.get::<_, String>(0)).ok()
            })
            .is_some_and(|goal| goal == k.0);
        if matches_goal {
            reset(tx, &k, "A task outcome or attention event arrived")?;
        }
    } else if kind == "message" && !k.3.is_empty() && sender != instance {
        let requester: String = tx
            .query_row("SELECT requester FROM tasks WHERE id=?1", [&k.3], |r| r.get(0))
            .map_err(|e| format!("work coordinator: {e}"))?;
        if sender == requester {
            reset(tx, &k, "The task coordinator supplied a new instruction")?;
        }
    }
    Ok(())
}

pub(super) fn begin(tx: &Connection, instance: &str, request: &str) -> Result<(), String> {
    let state = view(tx, instance)?;
    if state.is_null() {
        return Ok(());
    }
    if matches!(state["state"].as_str(), Some("NEEDS_INPUT" | "BLOCKED" | "STOPPED")) {
        return Err("work is waiting for new input or explicit recovery".into());
    }
    let k = current_key(tx, instance)?.ok_or("work has no live goal or pending user input")?;
    let unaddressed: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM context_entries WHERE instance_id=?1 AND epoch=?2
        AND compressed_by IS NULL AND idx=(SELECT MAX(idx) FROM context_entries WHERE instance_id=?1 AND epoch=?2
        AND compressed_by IS NULL) AND kind NOT IN ('assistant','runtime'))",
            rusqlite::params![instance, k.2],
            |r| r.get(0),
        )
        .map_err(|e| format!("work scheduling evidence: {e}"))?;
    if state["pending_reason"].as_str().unwrap_or("").is_empty() && !unaddressed {
        return Err("work has no unconsumed scheduling reason".into());
    }
    let frozen = context(tx, instance)?;
    tx.execute(
        "INSERT INTO work_requests(request_id,context_json) VALUES (?1,?2)",
        rusqlite::params![request, frozen.to_string()],
    )
    .map_err(|e| format!("freeze request work context: {e}"))?;
    tx.execute(
        "UPDATE work_states SET pending_reason='', state='RUNNING', revision=revision+1
        WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
        rusqlite::params![k.0, k.1, k.2, k.3],
    )
    .map_err(|e| format!("consume work reason: {e}"))?;
    Ok(())
}

fn set_state(tx: &Connection, k: &WorkKey, state: &str, reason: &str, pending: &str) -> Result<(), String> {
    tx.execute(
        "UPDATE work_states SET state=?5, reason=?6, pending_reason=?7, revision=revision+1
        WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
        rusqlite::params![k.0, k.1, k.2, k.3, state, reason, pending],
    )
    .map_err(|e| format!("work state: {e}"))?;
    Ok(())
}

fn repair(tx: &Connection, session: &str, instance: &str, reason: &str) -> Result<(), String> {
    let Some(k) = current_key(tx, instance)? else { return Ok(()) };
    tx.execute("UPDATE work_states SET empty_turns=empty_turns+1 WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
        rusqlite::params![k.0,k.1,k.2,k.3]).map_err(|e|format!("work repair counter: {e}"))?;
    let state = view(tx, instance)?;
    if state.is_null() {
        return Ok(());
    }
    if state["repair_turns"].as_i64().unwrap_or(0) > REPAIR_LIMIT {
        let reason = format!("No executable next action after bounded recovery: {reason}");
        set_state(tx, &k, "BLOCKED", &reason, "")?;
        stopped_notice(tx, &k, &reason)?;
        attention(tx, session, instance, &reason)?;
        event(tx, session, "work_blocked", instance, &json!({"goal_id":k.0,"reason":reason}))?;
    } else {
        let action=format!("{reason} Continue with an executable action, wait for an actual event, ask a material question with work_checkpoint, or finish with evidence. Repeated prose and plan edits do not count as progress.");
        set_state(tx, &k, "RUNNING", reason, &action)?;
        event(
            tx,
            session,
            "work_continuation",
            instance,
            &json!({"goal_id":k.0,"reason":action,"repair_turns":state["repair_turns"]}),
        )?;
    }
    Ok(())
}

fn checkpoint_problem(value: &Json) -> Option<&'static str> {
    if !value.is_object() || value.to_string().len() > CHECKPOINT_BYTES {
        return Some("work_checkpoint must be an object of at most 32 KiB");
    }
    if !matches!(value["action"].as_str(), Some("start_work" | "continue" | "ask_user" | "blocked")) {
        return Some("work_checkpoint.action must be start_work, continue, ask_user or blocked");
    }
    let allowed =
        ["action", "objective", "decisions", "completed", "remaining", "evidence", "next_step", "question", "reason"];
    if value.as_object().is_some_and(|object| object.keys().any(|key| !allowed.contains(&key.as_str()))) {
        return Some("unknown work checkpoint field");
    }
    for field in ["objective", "decisions", "completed", "remaining", "evidence", "next_step", "question", "reason"] {
        if let Some(v) = value.get(field) {
            if matches!(field, "decisions" | "completed" | "remaining" | "evidence") {
                if !v.as_array().is_some_and(|a| a.len() <= 128 && a.iter().all(Json::is_string)) {
                    return Some("work checkpoint lists must contain at most 128 strings");
                }
            } else if !v.is_string() {
                return Some("work checkpoint text fields must be strings");
            }
        }
    }
    let required = match value["action"].as_str() {
        Some("start_work") => "objective",
        Some("continue") => "next_step",
        Some("ask_user") => "question",
        _ => "reason",
    };
    if value[required].as_str().is_none_or(|s| s.trim().is_empty()) {
        return Some("work checkpoint requires objective, next_step, question or reason for its action");
    }
    None
}

pub(super) fn response(
    tx: &Connection,
    session: &str,
    instance: &str,
    decision: &str,
    params: &Json,
) -> Result<(), String> {
    if let Some(frozen) = admission::request(tx, params["request_id"].as_str().ok_or("work response request missing")?)?
    {
        return admission::response(tx, session, instance, decision, params, &frozen);
    }
    let state = view(tx, instance)?;
    let checkpoint = params.get("checkpoint");
    if state.is_null() {
        if checkpoint.is_some() {
            answer_dangling_calls(
                tx,
                instance,
                load_instance(tx, instance)?.1,
                "work_checkpoint is unavailable until user work is admitted",
            )?;
        }
        return Ok(());
    }
    let Some(k) = key(tx, instance)? else {
        if checkpoint.is_some() {
            answer_dangling_calls(
                tx,
                instance,
                load_instance(tx, instance)?.1,
                "The assignment is blocked; cancel and redispatch it before continuing",
            )?;
        }
        return Ok(());
    };
    if state["state"] == "STOPPED" {
        if checkpoint.is_some() {
            answer_dangling_calls(tx, instance, k.2, "Work is stopped; explicit user recovery is required")?;
        }
        return Ok(());
    }
    let request = params["request_id"].as_str().ok_or("response request missing")?;
    let frozen = tx
        .query_row("SELECT context_json FROM work_requests WHERE request_id=?1", [request], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| format!("response work scope: {e}"))?
        .and_then(|raw| serde_json::from_str::<Json>(&raw).ok())
        .unwrap_or(Json::Null);
    if frozen["goal_id"].as_str() != Some(k.0.as_str())
        || frozen["epoch"].as_i64() != Some(k.2)
        || frozen["task_id"].as_str() != Some(k.3.as_str())
    {
        if checkpoint.is_some() {
            answer_dangling_calls(
                tx,
                instance,
                k.2,
                "This checkpoint belongs to earlier work; reconcile the current assignment",
            )?;
        }
        return Ok(());
    }
    if let Some(value) = checkpoint {
        if value["action"] == "start_work" {
            answer_dangling_calls(
                tx,
                instance,
                k.2,
                "Work is already active; continue this goal instead of creating another",
            )?;
            return repair(tx, session, instance, "Keep the existing work goal and budget");
        }
        if let Some(problem) = checkpoint_problem(value) {
            answer_dangling_calls(tx, instance, k.2, problem)?;
            return repair(tx, session, instance, problem);
        }
        let mut saved = state["checkpoint"].clone();
        for (field, value) in value.as_object().ok_or("checkpoint is not an object")? {
            if field != "action" {
                saved[field] = value.clone();
            }
        }
        if saved.to_string().len() > CHECKPOINT_BYTES {
            answer_dangling_calls(tx, instance, k.2, "merged checkpoint exceeds 32 KiB; shorten the saved fields")?;
            return repair(tx, session, instance, "Checkpoint exceeded its storage bound");
        }
        tx.execute(
            "UPDATE work_states SET checkpoint_json=?5 WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
            rusqlite::params![k.0, k.1, k.2, k.3, saved.to_string()],
        )
        .map_err(|e| format!("save work checkpoint: {e}"))?;
        answer_dangling_calls(
            tx,
            instance,
            k.2,
            "Work checkpoint recorded as model claims; user inputs, checks, grants and usage remain authoritative.",
        )?;
        match value["action"].as_str().unwrap_or("") {
            "continue" => repair(tx, session, instance, "Checkpoint saved; execute the recorded next step")?,
            action => {
                let reason = value[if action == "ask_user" { "question" } else { "reason" }].as_str().unwrap_or("");
                let target = if action == "ask_user" { "NEEDS_INPUT" } else { "BLOCKED" };
                set_state(tx, &k, target, reason, "")?;
                let already_shown = params["entry"]["content"].as_str().unwrap_or("").trim() == reason.trim();
                append_context(
                    tx,
                    instance,
                    k.2,
                    if already_shown { "runtime" } else { "assistant" },
                    &json!({"role":if already_shown {"user"} else {"assistant"},"content":reason}),
                    Some(&format!("work-{}-{decision}", if action == "ask_user" { "question" } else { "blocked" })),
                    &json!([]),
                )?;
                attention(tx, session, instance, reason)?;
            }
        }
        event(
            tx,
            session,
            "work_checkpoint",
            instance,
            &json!({"goal_id":k.0,"action":value["action"],"decision_id":decision}),
        )?;
    } else if params.get("completion").is_some() {
        set_state(tx, &k, "VERIFYING", "Checking the completion claim", "")?;
    } else if params.get("wait").is_some() {
        let phase = load_instance(tx, instance)?.3;
        if phase == "WAITING" {
            set_state(tx, &k, "WAITING", "Waiting for registered events", "")?;
        } else {
            repair(tx, session, instance, "The requested wait was already satisfied; choose new work")?;
        }
    } else if params["intents"].as_array().is_none_or(Vec::is_empty) {
        repair(tx, session, instance, "The response ended while agreed work remains open")?;
    }
    Ok(())
}

pub(super) fn tools_consumed(
    tx: &Connection,
    session: &str,
    instance: &str,
    decision: &str,
    signature: &Json,
) -> Result<(), String> {
    let state = view(tx, instance)?;
    if state.is_null() || matches!(state["state"].as_str(), Some("STOPPED" | "BLOCKED" | "NEEDS_INPUT")) {
        return Ok(());
    }
    let Some(k) = key(tx, instance)? else { return Ok(()) };
    let source: Option<(Option<String>, i64, String)> = tx
        .query_row(
            "SELECT r.goal_id,r.epoch,w.context_json FROM decisions d
        JOIN model_requests r ON r.request_id=d.request_id JOIN work_requests w ON w.request_id=r.request_id
        WHERE d.decision_id=?1",
            [decision],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("tool work source: {e}"))?;
    let Some((goal, epoch, frozen)) = source else { return Ok(()) };
    let frozen: Json = serde_json::from_str(&frozen).map_err(|e| format!("tool work source JSON: {e}"))?;
    if goal.as_deref() != Some(&k.0) || epoch != k.2 || frozen["task_id"].as_str() != Some(&k.3) {
        return Ok(());
    }
    let raw: String = tx
        .query_row(
            "SELECT fingerprints_json FROM work_states WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
            rusqlite::params![k.0, k.1, k.2, k.3],
            |r| r.get(0),
        )
        .map_err(|e| format!("work fingerprints: {e}"))?;
    let mut fingerprints: Vec<String> =
        serde_json::from_str(&raw).map_err(|e| format!("work fingerprint JSON: {e}"))?;
    fingerprints.push(crate::kernel::args_hash(signature));
    if fingerprints.len() > 6 {
        fingerprints.remove(0);
    }
    let repeated = (1..=2).any(|period| {
        let n = fingerprints.len();
        n >= period * 3
            && fingerprints[n - period..] == fingerprints[n - 2 * period..n - period]
            && fingerprints[n - period..] == fingerprints[n - 3 * period..n - 2 * period]
    });
    tx.execute("UPDATE work_states SET fingerprints_json=?5, empty_turns=CASE WHEN ?6 THEN empty_turns ELSE 0 END,
        pending_reason='Tool results received', revision=revision+1 WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
        rusqlite::params![k.0,k.1,k.2,k.3,json!(fingerprints).to_string(),repeated]).map_err(|e|format!("work observation: {e}"))?;
    if repeated {
        repair(tx,session,instance,"Repeated actions returned the same observations; revise the approach or use an explicit event wait for intentional polling")?;
    }
    Ok(())
}

pub(super) fn resume(tx: &Connection, instance: &str) -> Result<(), String> {
    if let Some(k) = current_key(tx, instance)? {
        reset(tx, &k, "The user explicitly resumed work")?;
        if k.0.is_empty() {
            admission::resume_acceptance(tx, instance)?;
        }
    }
    Ok(())
}

pub(super) fn stop(tx: &Connection, instance: &str, reason: &str) -> Result<(), String> {
    if let Some(k) = current_key(tx, instance)? {
        set_state(tx, &k, "STOPPED", reason, "")?;
    }
    Ok(())
}

pub(super) fn attention(tx: &Connection, session: &str, instance: &str, reason: &str) -> Result<(), String> {
    let Some(k) = key(tx, instance)? else { return Ok(()) };
    if k.3.is_empty() {
        return Ok(());
    }
    attention_task(tx, session, &k.3, reason)
}

pub(super) fn attention_task(tx: &Connection, session: &str, task: &str, reason: &str) -> Result<(), String> {
    let (_, requester, instance, _) = load_task(tx, session, task)?;
    if requester == "user" || view(tx, &requester)?.is_null() {
        return Ok(());
    }
    let (_, epoch, lifecycle, _, _) = load_instance(tx, &requester)?;
    if lifecycle == "TERMINATED" {
        return Ok(());
    }
    queue_envelope(
        tx,
        session,
        &Outbox {
            sender: &instance,
            recipient: &requester,
            epoch,
            kind: "task_attention",
            correlation_id: Some(task),
            payload: &json!({"task_id":task,"reason":reason,"meaning":"Attention, not task completion. Inspect running operations before takeover."}),
        },
    )?;
    wake_attention(tx, session, &requester)
}

pub(super) fn unknown_operation(tx: &Connection, decision: &str, operation: &str) -> Result<(), String> {
    tx.execute(
        "UPDATE work_states SET state=CASE WHEN state='STOPPED' THEN state ELSE 'BLOCKED' END,
        pending_reason='',reason=?2,revision=revision+1
        WHERE (goal_id,instance_id,epoch,task_id) IN (
            SELECT r.goal_id,r.instance_id,r.epoch,json_extract(w.context_json,'$.task_id')
            FROM decisions d JOIN model_requests r ON r.request_id=d.request_id
            JOIN work_requests w ON w.request_id=r.request_id WHERE d.decision_id=?1)",
        rusqlite::params![
            decision,
            format!(
                "Operation {operation} has an unknown outcome; inspect receipts and running processes before recovery"
            )
        ],
    )
    .map_err(|e| format!("unknown work outcome: {e}"))?;
    Ok(())
}

pub(super) fn wake(tx: &Connection, instance: &str, epoch: i64, reason: &str) -> Result<(), String> {
    if view(tx, instance)?["state"] != "WAITING" {
        return Ok(());
    }
    if let Some(k) = key(tx, instance)?.filter(|k| k.2 == epoch) {
        set_state(tx, &k, "RUNNING", "", reason)?;
    }
    Ok(())
}

pub(super) fn fork(tx: &Connection, keep: &str) -> Result<(), String> {
    let Some(settings) = policy(tx)? else { return Ok(()) };
    if settings["leader_id"] != keep {
        return Err("a durable-work fork must keep the configured leader".into());
    }
    let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE status IN ('DISPATCH_COMMITTED','RUNNING','OUTCOME_UNKNOWN') AND quiescence_json IS NULL)",[],|r|r.get(0))
        .map_err(|e|format!("fork execution evidence: {e}"))?;
    if active {
        return Err("tracked execution has no stop evidence; recover or stop it before forking durable work".into());
    }
    admission::carry_for_fork(tx, keep)?;
    tx.execute("UPDATE goals SET unknown_usage=unknown_usage+(SELECT COUNT(*) FROM model_requests r WHERE r.goal_id=goals.id AND r.status='PENDING' AND r.selected_attempt_id IS NULL)",[])
        .map_err(|e|format!("fork pending usage: {e}"))?;
    tx.execute("UPDATE work_states SET policy_json=json_set(policy_json,'$.inherited_execution',1) WHERE EXISTS(SELECT 1 FROM operations o WHERE o.goal_id=work_states.goal_id)",[])
        .map_err(|e|format!("fork inherited evidence: {e}"))?;
    for table in ["work_states", "work_inputs"] {
        tx.execute(&format!("DELETE FROM {table} WHERE instance_id<>?1"), [keep])
            .map_err(|e| format!("fork work ownership: {e}"))?;
    }
    tx.execute("DELETE FROM work_states WHERE task_id<>''", []).map_err(|e| format!("fork task work: {e}"))?;
    tx.execute("DELETE FROM work_requests", []).map_err(|e| format!("fork work requests: {e}"))?;
    tx.execute("UPDATE work_states SET state=CASE WHEN state IN ('RUNNING','WAITING') THEN 'STOPPED' ELSE state END,
        reason=CASE WHEN state IN ('RUNNING','WAITING') THEN 'Conversation fork; provide explicit input before continuing' ELSE reason END,
        pending_reason='',revision=revision+1",[]).map_err(|e|format!("fork work stop: {e}"))?;
    answer_dangling_calls(tx,keep,load_instance(tx,keep)?.1,"Historical call from the source session: its execution was not copied. Inspect the source receipts before continuing; this note does not establish its outcome.")?;
    Ok(())
}

pub(super) fn wake_attention(tx: &Connection, session: &str, instance: &str) -> Result<(), String> {
    if view(tx, instance)?.is_null() {
        return Ok(());
    }
    let (_, epoch, _, phase, _) = load_instance(tx, instance)?;
    if phase != "WAITING" {
        return Ok(());
    }
    let Some(k) = key(tx, instance)? else { return Ok(()) };
    let attention: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM envelopes e JOIN tasks t ON t.id=e.correlation_id
        WHERE e.recipient=?1 AND e.epoch=?2 AND e.state='ACCEPTED' AND t.goal_id=?3
        AND (e.kind='task_attention' OR (e.kind='task_result' AND json_extract(e.payload_json,'$.status')='BLOCKED')))",
            rusqlite::params![instance, epoch, k.0],
            |r| r.get(0),
        )
        .map_err(|e| format!("work attention query: {e}"))?;
    let workspace: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM envelopes WHERE recipient=?1 AND epoch=?2 AND state='ACCEPTED'
        AND kind='workspace_available' AND sender='runtime' AND correlation_id=?3)",
            rusqlite::params![instance, epoch, crate::kernel::args_hash(&json!(k))],
            |r| r.get(0),
        )
        .map_err(|e| format!("workspace wake query: {e}"))?;
    if !attention && !workspace {
        return Ok(());
    }
    let mut stmt = tx
        .prepare("SELECT id FROM waits WHERE instance_id=?1 AND epoch=?2 AND status='PENDING'")
        .map_err(|e| format!("attention waits: {e}"))?;
    let waits = stmt
        .query_map(rusqlite::params![instance, epoch], |r| r.get::<_, String>(0))
        .map_err(|e| format!("attention waits query: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("attention waits collect: {e}"))?;
    for wait in &waits {
        tx.execute("UPDATE waits SET status='CANCELLED' WHERE id=?1", [wait])
            .map_err(|e| format!("attention wait: {e}"))?;
    }
    answer_closed_waits(
        tx,
        instance,
        epoch,
        &waits,
        if workspace {
            "Wait superseded by a recorded process stop; its execution outcome has not been rewritten."
        } else {
            "Wait superseded by task attention; the task has not necessarily completed."
        },
    )?;
    tx.execute("UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='WAITING'", [instance])
        .map_err(|e| format!("attention wake: {e}"))?;
    wake(
        tx,
        instance,
        epoch,
        if workspace { "A conflicting process group has stopped" } else { "A task needs coordinator attention" },
    )?;
    event(
        tx,
        session,
        "work_attention",
        instance,
        &json!({"waits":waits,"reason":if workspace {"workspace_available"} else {"task_attention"}}),
    )?;
    Ok(())
}

fn stopped_notice(tx: &Connection, k: &WorkKey, reason: &str) -> Result<(), String> {
    let revision = view(tx, &k.1)?["revision"].as_i64().unwrap_or(0);
    append_context(
        tx,
        &k.1,
        k.2,
        "runtime",
        &json!({"role":"user","content":format!("runtime: work stopped: {reason}")}),
        Some(&format!("work-stopped-{}-{}-{revision}", k.0, k.3)),
        &json!([]),
    )?;
    Ok(())
}

fn evidence_problem(tx: &Connection, k: &WorkKey, evidence: &Json) -> Result<Option<&'static str>, String> {
    let used_tools: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM operations o JOIN decisions d ON d.decision_id=o.decision_id
        JOIN model_requests r ON r.request_id=d.request_id LEFT JOIN work_requests w ON w.request_id=r.request_id
        WHERE r.instance_id=?1 AND r.goal_id=?2 AND (?3='' OR json_extract(w.context_json,'$.task_id')=?3))",
            rusqlite::params![k.1, k.0, k.3],
            |r| r.get(0),
        )
        .map_err(|e| format!("completion tool evidence: {e}"))?;
    let inherited: bool = tx
        .query_row(
            "SELECT COALESCE(json_extract(policy_json,'$.inherited_execution'),0) FROM work_states
        WHERE goal_id=?1 AND instance_id=?2 AND epoch=?3 AND task_id=?4",
            rusqlite::params![k.0, k.1, k.2, k.3],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("inherited work evidence: {e}"))?
        .unwrap_or(false);
    if !used_tools && !inherited {
        return Ok(None);
    }
    let mut grounded:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM operations o JOIN decisions d ON d.decision_id=o.decision_id
        JOIN model_requests r ON r.request_id=d.request_id WHERE r.goal_id=?1 AND r.request_ref='required_check' AND o.status='SUCCEEDED')",
        [&k.0],|r|r.get(0)).map_err(|e|format!("completion check evidence: {e}"))?;
    let refs = evidence.as_array().map(Vec::as_slice).unwrap_or(&[]);
    if refs.len() > 128 {
        return Ok(Some("Completion evidence exceeds 128 references"));
    }
    for reference in refs.iter().filter_map(Json::as_str) {
        let (kind, id) = if let Some(id) = reference.strip_prefix("operation:") {
            ("operation", id)
        } else if let Some(id) = reference.strip_prefix("tool:") {
            ("tool", id)
        } else if let Some(id) = reference.strip_prefix("task:") {
            ("task", id)
        } else {
            continue;
        };
        let valid:bool=if kind=="task" {
            tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND goal_id=?2 AND requester=?3 AND status='SUCCEEDED')",
                rusqlite::params![id,k.0,k.1],|r|r.get(0))
        } else {
            tx.query_row("SELECT COALESCE((SELECT o.status='SUCCEEDED' FROM operations o JOIN decisions d ON d.decision_id=o.decision_id
                JOIN model_requests r ON r.request_id=d.request_id LEFT JOIN work_requests w ON w.request_id=r.request_id
                WHERE r.instance_id=?1 AND r.goal_id=?2 AND (?3='' OR json_extract(w.context_json,'$.task_id')=?3)
                AND ((?4='operation' AND o.operation_id=?5) OR (?4='tool' AND json_extract(o.intent_json,'$.call_id')=?5))
                ORDER BY o.rowid DESC LIMIT 1),0)",rusqlite::params![k.1,k.0,k.3,kind,id],|r|r.get(0))
        }.map_err(|e|format!("completion evidence reference: {e}"))?;
        if !valid {
            return Ok(Some("A cited execution or task reference is not a successful outcome in this work"));
        }
        grounded = true;
    }
    Ok((!grounded).then_some("Operational work needs recorded completion evidence: cite a successful tool:CALL_ID, operation:OPERATION_ID or task:TASK_ID in finish.evidence, and reconcile it with the user's actual outcome"))
}

pub(super) fn completion(
    tx: &Connection,
    session: &str,
    instance: &str,
    outcome: &str,
    summary: &str,
    evidence: &Json,
) -> Result<bool, String> {
    let state = view(tx, instance)?;
    if state.is_null() {
        return Ok(false);
    }
    let k = key(tx, instance)?.ok_or("completion work missing")?;
    if matches!(state["state"].as_str(), Some("STOPPED" | "NEEDS_INPUT" | "BLOCKED")) {
        answer_dangling_calls(tx, instance, k.2, "Work awaits input or recovery before settlement")?;
        tx.execute(
            "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
            [instance],
        )
        .map_err(|e| format!("stop completion: {e}"))?;
        return Ok(true);
    }
    let frozen:Option<Option<String>>=tx.query_row("SELECT w.context_json FROM decisions d JOIN model_requests r ON r.request_id=d.request_id
        LEFT JOIN work_requests w ON w.request_id=r.request_id WHERE r.instance_id=?1 AND d.completion_json IS NOT NULL ORDER BY d.rowid DESC LIMIT 1",
        [instance],|r|r.get(0)).optional().map_err(|e|format!("completion work scope: {e}"))?;
    if let Some(frozen) = frozen {
        let frozen: Json = frozen.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or(Json::Null);
        let latest: Option<String> = tx
            .query_row(
                "SELECT c.id FROM work_inputs w JOIN context_entries c
            ON c.envelope_id=w.envelope_id AND c.instance_id=w.instance_id AND c.epoch=w.epoch
            WHERE w.goal_id=?1 AND w.instance_id=?2 ORDER BY w.rowid DESC LIMIT 1",
                rusqlite::params![k.0, instance],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("completion input scope: {e}"))?;
        let input_seen = latest.is_none_or(|latest| {
            frozen["recent_user_inputs"]
                .as_array()
                .is_some_and(|inputs| inputs.iter().any(|input| input["context_ref"].as_str() == Some(latest.as_str())))
        });
        if frozen["goal_id"] != k.0 || frozen["epoch"] != k.2 || frozen["task_id"] != k.3 || !input_seen {
            answer_dangling_calls(tx,instance,k.2,"Completion came from an earlier work scope or omitted newer user input; reconcile the current work first")?;
            tx.execute(
                "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
                [instance],
            )
            .map_err(|e| format!("defer stale completion: {e}"))?;
            set_state(
                tx,
                &k,
                "RUNNING",
                "Completion scope changed",
                "Reconcile the current assignment and user input before finishing",
            )?;
            return Ok(true);
        }
    }
    if outcome == "SUCCEEDED" {
        let unread:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM envelopes WHERE recipient=?1 AND epoch=?2 AND kind='user_input' AND state IN ('ACCEPTED','DEFERRED'))",
            rusqlite::params![instance,k.2],|r|r.get(0)).map_err(|e|format!("completion queued input: {e}"))?;
        if unread {
            answer_dangling_calls(
                tx,
                instance,
                k.2,
                "New user input arrived before settlement; incorporate it into this work before finishing",
            )?;
            tx.execute(
                "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
                [instance],
            )
            .map_err(|e| format!("defer completion for input: {e}"))?;
            tx.execute("UPDATE envelopes SET state='ACCEPTED' WHERE recipient=?1 AND epoch=?2 AND kind='user_input' AND state='DEFERRED'",
                rusqlite::params![instance,k.2]).map_err(|e|format!("promote completion follow-up: {e}"))?;
            set_state(
                tx,
                &k,
                "RUNNING",
                "New user input awaits admission",
                "Incorporate the queued user input before settling this work",
            )?;
            return Ok(true);
        }
        let unsettled: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM operations o JOIN decisions d ON d.decision_id=o.decision_id
            JOIN model_requests r ON r.request_id=d.request_id LEFT JOIN work_requests w ON w.request_id=r.request_id
            WHERE o.goal_id=?1 AND o.quiescence_json IS NULL AND (?2='' OR json_extract(w.context_json,'$.task_id')=?2)
            AND o.status IN ('PREPARED','DISPATCH_COMMITTED','RUNNING','OUTCOME_UNKNOWN'))",
                rusqlite::params![k.0, k.3],
                |r| r.get(0),
            )
            .map_err(|e| format!("completion active effects: {e}"))?;
        let open: i64 = if k.3.is_empty() {
            tx.query_row(
                "SELECT COUNT(*) FROM tasks WHERE goal_id=?1 AND status NOT IN ('SUCCEEDED','FAILED','CANCELLED')",
                [&k.0],
                |r| r.get(0),
            )
            .map_err(|e| format!("completion open tasks: {e}"))?
        } else {
            0
        };
        let evidence_problem = evidence_problem(tx, &k, evidence)?;
        if unsettled
            || open > 0
            || state["checkpoint"]["remaining"].as_array().is_some_and(|a| !a.is_empty())
            || evidence_problem.is_some()
        {
            let reason = if unsettled {
                "Completion refused: tracked execution is still running or has an unresolved outcome; inspect its receipts and stop evidence."
            } else {
                evidence_problem.unwrap_or(
                    "Completion refused: outstanding tasks or checkpoint items remain; reconcile their evidence first.",
                )
            };
            answer_dangling_calls(tx, instance, k.2, reason)?;
            tx.execute(
                "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
                [instance],
            )
            .map_err(|e| format!("completion repair: {e}"))?;
            repair(tx, session, instance, reason)?;
            return Ok(true);
        }
    }
    if outcome == "BLOCKED" && k.3.is_empty() {
        set_state(tx, &k, "BLOCKED", summary, "")?;
        stopped_notice(tx, &k, summary)?;
        answer_dangling_calls(
            tx,
            instance,
            k.2,
            "Work is blocked; the goal stays active with its existing budget and evidence.",
        )?;
        tx.execute(
            "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
            [instance],
        )
        .map_err(|e| format!("blocked work phase: {e}"))?;
        event(tx, session, "work_blocked", instance, &json!({"goal_id":k.0,"reason":summary}))?;
        return Ok(true);
    }
    if outcome == "BLOCKED" {
        attention(tx, session, instance, summary)?;
    }
    set_state(tx, &k, if outcome == "BLOCKED" { "BLOCKED" } else { "COMPLETE" }, summary, "")?;
    Ok(false)
}
