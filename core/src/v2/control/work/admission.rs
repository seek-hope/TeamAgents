//! Admission is scoped to pending user inputs; informational replies create no goal.
//! The empty goal key denotes input whose work intent has not yet been decided.

use super::*;

pub(super) fn queue(
    tx: &Connection,
    session: &str,
    instance: &str,
    epoch: i64,
    envelope: &str,
    settings: &Json,
) -> Result<(), String> {
    let existing: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM work_inputs WHERE instance_id=?1 AND state='PENDING')",
            [instance],
            |r| r.get(0),
        )
        .map_err(|e| format!("pending work input: {e}"))?;
    let k = (String::new(), instance.into(), epoch, String::new());
    ensure(tx, &k)?;
    if !existing {
        tx.execute("UPDATE work_states SET checkpoint_json='{}',policy_json=?3 WHERE goal_id='' AND instance_id=?1 AND epoch=?2",
            rusqlite::params![instance,epoch,settings.to_string()]).map_err(|e|format!("pin admission policy: {e}"))?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO work_inputs(goal_id,instance_id,epoch,envelope_id,state) VALUES ('',?1,?2,?3,'PENDING')",
        rusqlite::params![instance, epoch, envelope],
    )
    .map_err(|e| format!("queue work admission: {e}"))?;
    event(tx, session, "work_input_pending", instance, &json!({"envelope_id":envelope}))?;
    if load_instance(tx, instance)?.2 != "ACTIVE" {
        set_state(tx, &k, "STOPPED", "Input recorded; explicitly resume the instance before interpreting it", "")?;
        return Ok(());
    }
    reset(tx,&k,"Interpret the new input: answer informational follow-ups directly, or use work_checkpoint start_work for a new task")?;
    let payload: String = tx
        .query_row("SELECT payload_json FROM envelopes WHERE id=?1", [envelope], |r| r.get(0))
        .map_err(|e| format!("explicit admission checks: {e}"))?;
    let payload: Json = serde_json::from_str(&payload).map_err(|e| format!("explicit admission input: {e}"))?;
    if payload["checks"].as_array().is_some_and(|checks| !checks.is_empty()) {
        activate(tx, session, instance, &view(tx, instance)?, &json!({}), "user_acceptance")?;
    }
    Ok(())
}

pub(super) fn resume_acceptance(tx: &Connection, instance: &str) -> Result<(), String> {
    let checks: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM work_inputs w JOIN envelopes e ON e.id=w.envelope_id
        WHERE w.instance_id=?1 AND w.state='PENDING' AND json_array_length(json_extract(e.payload_json,'$.checks'))>0)",
            [instance],
            |r| r.get(0),
        )
        .map_err(|e| format!("pending acceptance on resume: {e}"))?;
    if checks {
        activate(tx, &load_instance(tx, instance)?.0, instance, &view(tx, instance)?, &json!({}), "user_acceptance")?;
    }
    Ok(())
}

fn inputs(tx: &Connection, instance: &str) -> Result<Vec<Json>, String> {
    let mut stmt = tx
        .prepare(
            "SELECT w.envelope_id,w.epoch,c.id,c.message_json,c.created FROM work_inputs w
        JOIN context_entries c ON c.instance_id=w.instance_id AND c.epoch=w.epoch AND c.envelope_id=w.envelope_id
        WHERE w.instance_id=?1 AND w.state='PENDING' ORDER BY w.rowid",
        )
        .map_err(|e| format!("admission inputs: {e}"))?;
    let rows=stmt.query_map([instance],|r|{
        let raw:String=r.get(3)?;
        let message:Json=serde_json::from_str(&raw).unwrap_or(Json::Null);
        let text=message["content"].as_str().unwrap_or("");
        Ok(json!({"envelope_id":r.get::<_,String>(0)?,"epoch":r.get::<_,i64>(1)?,"context_ref":r.get::<_,String>(2)?,
            "text":text.chars().take(1500).collect::<String>(),"truncated":text.chars().count()>1500,
            "created":r.get::<_,f64>(4)?,"provenance":"user_input"}))
    }).map_err(|e|format!("admission inputs query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("admission inputs collect: {e}"))
}

pub(super) fn view(tx: &Connection, instance: &str) -> Result<Json, String> {
    let inputs = inputs(tx, instance)?;
    let Some(first) = inputs.first() else { return Ok(Json::Null) };
    let epoch = load_instance(tx, instance)?.1;
    let mut previous=tx.prepare("SELECT g.id,g.status,g.original_request_ref,g.known_usage_json,g.unknown_usage
        FROM goals g WHERE EXISTS(SELECT 1 FROM work_states w WHERE w.goal_id=g.id AND w.instance_id=?1 AND w.task_id='')
        ORDER BY g.rowid DESC LIMIT 4").map_err(|e|format!("previous work status: {e}"))?;
    let previous=previous.query_map([instance],|r|{
        let usage:String=r.get(3)?;
        Ok(json!({"goal_id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"original_request_ref":r.get::<_,String>(2)?,
            "known_usage":serde_json::from_str::<Json>(&usage).unwrap_or(Json::Null),"unknown_usage":r.get::<_,i64>(4)?,"provenance":"runtime"}))
    }).map_err(|e|format!("previous work status query: {e}"))?.collect::<Result<Vec<_>,_>>().map_err(|e|format!("previous work status rows: {e}"))?;
    tx.query_row("SELECT state,checkpoint_json,pending_reason,empty_turns,revision,reason,policy_json
        FROM work_states WHERE goal_id='' AND instance_id=?1 AND epoch=?2 AND task_id=''",
        rusqlite::params![instance,epoch],|r|{
            let checkpoint:String=r.get(1)?;
            let policy:String=r.get(6)?;
            let policy:Json=serde_json::from_str(&policy).unwrap_or(Json::Null);
            Ok(json!({"policy_version":1,"admission":true,"admission_id":first["envelope_id"],"goal_id":null,
                "instance_id":instance,"epoch":epoch,"task_id":"","state":r.get::<_,String>(0)?,
                "checkpoint":serde_json::from_str::<Json>(&checkpoint).unwrap_or(json!({})),"checkpoint_provenance":"model_claim",
                "pending_reason":r.get::<_,String>(2)?,"repair_turns":r.get::<_,i64>(3)?,"revision":r.get::<_,i64>(4)?,
                "reason":r.get::<_,String>(5)?,"limits":policy["limits"],"previous_goals":previous,
                "carried_usage":policy["admission_carry"]["known_usage"],"carried_unknown_usage":policy["admission_carry"]["unknown_usage"],"original_input":first,
                "recent_user_inputs":inputs.iter().skip(inputs.len().saturating_sub(16)).collect::<Vec<_>>() }))
        }).optional().map(|value|value.unwrap_or(Json::Null)).map_err(|e|format!("admission view: {e}"))
}

pub(super) fn request(tx: &Connection, request: &str) -> Result<Option<Json>, String> {
    let saved: Option<String> = tx
        .query_row("SELECT context_json FROM work_requests WHERE request_id=?1", [request], |r| r.get(0))
        .optional()
        .map_err(|e| format!("admission request: {e}"))?;
    saved
        .map(|raw| serde_json::from_str::<Json>(&raw).map_err(|e| format!("admission request JSON: {e}")))
        .transpose()
        .map(|value| value.filter(|value| value["admission"] == true))
}

pub(super) fn budget(tx: &Connection, instance: &str, estimate: i64) -> Result<Option<String>, String> {
    let state = view(tx, instance)?;
    if state.is_null() {
        return Ok(None);
    }
    let mut stmt = tx
        .prepare(
            "SELECT a.usage_json FROM attempts a JOIN work_requests w ON w.request_id=a.request_id
        WHERE a.status='COMPLETE' AND json_extract(w.context_json,'$.admission_id')=?1",
        )
        .map_err(|e| format!("admission usage: {e}"))?;
    let values = stmt
        .query_map([state["admission_id"].as_str()], |r| r.get::<_, String>(0))
        .map_err(|e| format!("admission usage query: {e}"))?;
    let mut used = crate::kernel::Usage::from_json(&json!({"usage":state["carried_usage"]})).unwrap_or_default().total;
    for raw in values {
        let usage: Json =
            serde_json::from_str(&raw.map_err(|e| format!("admission usage row: {e}"))?).unwrap_or(Json::Null);
        used = used.saturating_add(crate::kernel::Usage::from_json(&json!({"usage":usage})).unwrap_or_default().total);
    }
    if let Some(max) = state["limits"]["max_total_tokens"].as_u64() {
        if used.saturating_add(estimate.max(0) as u64) > max {
            return Ok(Some(format!("Pending work budget exceeded: known {used} + estimate {estimate} > {max}")));
        }
    }
    if let Some(minutes) = state["limits"]["deadline_minutes"].as_u64() {
        if crate::models::now() >= state["original_input"]["created"].as_f64().unwrap_or(0.0) + minutes as f64 * 60.0 {
            return Ok(Some("Pending work deadline passed; explicit user recovery is required".into()));
        }
    }
    Ok(None)
}

pub(super) fn carry_for_fork(tx: &Connection, instance: &str) -> Result<(), String> {
    let state = view(tx, instance)?;
    if state.is_null() {
        return Ok(());
    }
    let mut usage = crate::kernel::Usage::from_json(&json!({"usage":state["carried_usage"]})).unwrap_or_default();
    let mut stmt = tx
        .prepare(
            "SELECT a.usage_json FROM attempts a JOIN work_requests w ON w.request_id=a.request_id
        WHERE a.status='COMPLETE' AND json_extract(w.context_json,'$.admission_id')=?1",
        )
        .map_err(|e| format!("fork admission usage: {e}"))?;
    let rows = stmt
        .query_map([state["admission_id"].as_str()], |r| r.get::<_, String>(0))
        .map_err(|e| format!("fork admission usage query: {e}"))?;
    for row in rows {
        let raw: Json = serde_json::from_str(&row.map_err(|e| format!("fork admission usage row: {e}"))?)
            .map_err(|e| format!("fork admission usage JSON: {e}"))?;
        let value = crate::kernel::Usage::from_json(&json!({"usage":raw})).unwrap_or_default();
        usage.prompt = usage.prompt.saturating_add(value.prompt);
        usage.completion = usage.completion.saturating_add(value.completion);
        usage.total = usage.total.saturating_add(value.total);
        usage.cache_hit = usage.cache_hit.saturating_add(value.cache_hit);
        usage.cache_miss = usage.cache_miss.saturating_add(value.cache_miss);
        usage.cache_write = usage.cache_write.saturating_add(value.cache_write);
    }
    let unknown:i64=tx.query_row("SELECT COALESCE(SUM(w.unknown_usage + CASE WHEN r.status='PENDING' AND r.selected_attempt_id IS NULL THEN 1 ELSE 0 END),0)
        FROM work_requests w JOIN model_requests r ON r.request_id=w.request_id WHERE json_extract(w.context_json,'$.admission_id')=?1",
        [state["admission_id"].as_str()],|r|r.get(0)).map_err(|e|format!("fork unknown admission usage: {e}"))?;
    let raw: String = tx
        .query_row(
            "SELECT policy_json FROM work_states WHERE goal_id='' AND instance_id=?1 AND epoch=?2",
            rusqlite::params![instance, state["epoch"].as_i64()],
            |r| r.get(0),
        )
        .map_err(|e| format!("fork admission policy: {e}"))?;
    let mut policy: Json = serde_json::from_str(&raw).map_err(|e| format!("fork admission policy JSON: {e}"))?;
    policy["admission_carry"] = json!({"known_usage":usage,"unknown_usage":unknown+state["carried_unknown_usage"].as_i64().unwrap_or(0),"provenance":"fork_snapshot"});
    tx.execute(
        "UPDATE work_states SET policy_json=?3 WHERE goal_id='' AND instance_id=?1 AND epoch=?2",
        rusqlite::params![instance, state["epoch"].as_i64(), policy.to_string()],
    )
    .map_err(|e| format!("carry fork admission: {e}"))?;
    Ok(())
}

fn close(tx: &Connection, instance: &str, epoch: i64) -> Result<(), String> {
    tx.execute("UPDATE work_inputs SET state='ANSWERED' WHERE instance_id=?1 AND state='PENDING'", [instance])
        .map_err(|e| format!("answer informational input: {e}"))?;
    set_state(
        tx,
        &(String::new(), instance.into(), epoch, String::new()),
        "COMPLETE",
        "Informational input answered",
        "",
    )
}

fn activate(
    tx: &Connection,
    session: &str,
    instance: &str,
    state: &Json,
    checkpoint: &Json,
    provenance: &str,
) -> Result<(), String> {
    let inputs = inputs(tx, instance)?;
    let first = inputs.first().ok_or("new work requires pending user input")?;
    let epoch = load_instance(tx, instance)?.1;
    let goal = format!(
        "goal-input-{}",
        crate::kernel::args_hash(&json!([session, instance, first["epoch"], first["envelope_id"]]))
    );
    let mut limits = state["limits"].clone();
    let minutes =
        limits.as_object_mut().and_then(|limits| limits.remove("deadline_minutes")).and_then(|value| value.as_u64());
    let mut params =
        json!({"id":goal,"instance_id":instance,"original_request_ref":first["envelope_id"],"limits":limits});
    if let Some(minutes) = minutes {
        params["deadline"] = json!(first["created"].as_f64().unwrap_or(0.0) + minutes as f64 * 60.0);
    }
    create_goal(tx, session, &params, &Identity::System)?;
    let k = (goal.clone(), instance.into(), epoch, String::new());
    ensure(tx, &k)?;
    let mut saved = state["checkpoint"].clone();
    for (key, value) in checkpoint.as_object().ok_or("work admission checkpoint must be an object")? {
        if key != "action" {
            saved[key] = value.clone();
        }
    }
    if saved.to_string().len() > CHECKPOINT_BYTES {
        return Err("merged admission checkpoint exceeds 32 KiB".into());
    }
    tx.execute(
        "UPDATE work_states SET checkpoint_json=?3 WHERE goal_id=?1 AND instance_id=?2 AND epoch=?4",
        rusqlite::params![goal, instance, saved.to_string(), epoch],
    )
    .map_err(|e| format!("admit work checkpoint: {e}"))?;
    let mut stmt = tx
        .prepare(
            "SELECT w.request_id,w.unknown_usage FROM work_requests w
        WHERE json_extract(w.context_json,'$.admission_id')=?1 AND w.billed_goal_id IS NULL",
        )
        .map_err(|e| format!("admission billing: {e}"))?;
    let requests = stmt
        .query_map([state["admission_id"].as_str()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| format!("admission billing query: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("admission billing collect: {e}"))?;
    settle_usage(tx, &goal, &state["carried_usage"])?;
    tx.execute(
        "UPDATE goals SET unknown_usage=unknown_usage+?2 WHERE id=?1",
        rusqlite::params![goal, state["carried_unknown_usage"].as_i64().unwrap_or(0)],
    )
    .map_err(|e| format!("carry admission usage into goal: {e}"))?;
    for (request, unknown) in &requests {
        let mut attempts = tx
            .prepare("SELECT usage_json FROM attempts WHERE request_id=?1 AND status='COMPLETE'")
            .map_err(|e| format!("admission attempts: {e}"))?;
        let usages = attempts
            .query_map([request], |r| r.get::<_, String>(0))
            .map_err(|e| format!("admission attempts query: {e}"))?;
        for usage in usages {
            let usage: Json = serde_json::from_str(&usage.map_err(|e| format!("admission attempt row: {e}"))?)
                .map_err(|e| format!("admission usage JSON: {e}"))?;
            settle_usage(tx, &goal, &usage)?;
        }
        tx.execute("UPDATE work_requests SET billed_goal_id=?2 WHERE request_id=?1", rusqlite::params![request, goal])
            .map_err(|e| format!("assign admission billing: {e}"))?;
        tx.execute("UPDATE goals SET unknown_usage=unknown_usage+?2 WHERE id=?1", rusqlite::params![goal, unknown])
            .map_err(|e| format!("assign unknown admission usage: {e}"))?;
    }
    for input in &inputs {
        tx.execute(
            "UPDATE work_inputs SET goal_id=?2,state='ASSIGNED' WHERE instance_id=?1 AND envelope_id=?3",
            rusqlite::params![instance, goal, input["envelope_id"].as_str()],
        )
        .map_err(|e| format!("assign work input: {e}"))?;
        super::admit_input(tx, session, instance, input["envelope_id"].as_str().ok_or("admission input id missing")?)?;
    }
    set_state(tx, &(String::new(), instance.into(), epoch, String::new()), "COMPLETE", "New work admitted", "")?;
    event(
        tx,
        session,
        "work_started",
        instance,
        &json!({"goal_id":goal,"input_ref":first["envelope_id"],"billed_requests":requests.len(),"intent_provenance":provenance}),
    )?;
    Ok(())
}

pub(super) fn response(
    tx: &Connection,
    session: &str,
    instance: &str,
    decision: &str,
    params: &Json,
    frozen: &Json,
) -> Result<(), String> {
    let state = view(tx, instance)?;
    let epoch = load_instance(tx, instance)?.1;
    if state.is_null() || state["admission_id"] != frozen["admission_id"] || budget_goal(tx, instance)?.is_some() {
        if !state.is_null() && state["admission_id"] == frozen["admission_id"] {
            tx.execute(
                "UPDATE work_inputs SET state='SUPERSEDED' WHERE instance_id=?1 AND state='PENDING'",
                [instance],
            )
            .map_err(|e| format!("supersede pending admission: {e}"))?;
            set_state(
                tx,
                &(String::new(), instance.into(), epoch, String::new()),
                "STOPPED",
                "A user-selected goal superseded this admission",
                "",
            )?;
        }
        tx.execute(
            "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
            [instance],
        )
        .map_err(|e| format!("close superseded admission: {e}"))?;
        answer_dangling_calls(
            tx,
            instance,
            epoch,
            "This input admission has been superseded; use the current work context",
        )?;
        return Ok(());
    }
    if state["state"] == "STOPPED" {
        answer_dangling_calls(tx, instance, epoch, "Work is stopped; explicit user recovery is required")?;
        return Ok(());
    }
    if params["intents"].as_array().is_some_and(|intents| !intents.is_empty()) || params.get("wait").is_some() {
        answer_dangling_calls(tx,instance,epoch,"Admit a new task with work_checkpoint start_work before using tools or waiting. Informational follow-ups need only an answer.")?;
        return repair(tx, session, instance, "Work intent must be admitted before execution");
    }
    if let Some(checkpoint) = params.get("checkpoint") {
        if let Some(problem) = checkpoint_problem(checkpoint) {
            answer_dangling_calls(tx, instance, epoch, problem)?;
            return repair(tx, session, instance, problem);
        }
        let mut merged = state["checkpoint"].clone();
        for (field, value) in checkpoint.as_object().ok_or("admission checkpoint object required")? {
            if field != "action" {
                merged[field] = value.clone();
            }
        }
        if merged.to_string().len() > CHECKPOINT_BYTES {
            answer_dangling_calls(tx, instance, epoch, "Merged checkpoint exceeds 32 KiB; shorten the saved fields")?;
            return repair(tx, session, instance, "Admission checkpoint exceeded its storage bound");
        }
        match checkpoint["action"].as_str().unwrap_or("") {
            "start_work" => {
                activate(tx, session, instance, &state, checkpoint, "model_decision")?;
                answer_dangling_calls(tx,instance,epoch,"New user work admitted with its original inputs, limits and acceptance checks; execute the next step")?;
            }
            "ask_user" | "blocked" => {
                let ask = checkpoint["action"] == "ask_user";
                let reason = checkpoint[if ask { "question" } else { "reason" }].as_str().unwrap_or("");
                let mut saved = state["checkpoint"].clone();
                for (field, value) in checkpoint.as_object().ok_or("admission checkpoint object required")? {
                    if field != "action" {
                        saved[field] = value.clone();
                    }
                }
                if saved.to_string().len() > CHECKPOINT_BYTES {
                    return Err("merged admission checkpoint exceeds 32 KiB".into());
                }
                tx.execute(
                    "UPDATE work_states SET checkpoint_json=?3 WHERE goal_id='' AND instance_id=?1 AND epoch=?2",
                    rusqlite::params![instance, epoch, saved.to_string()],
                )
                .map_err(|e| format!("admission clarification: {e}"))?;
                set_state(
                    tx,
                    &(String::new(), instance.into(), epoch, String::new()),
                    if ask { "NEEDS_INPUT" } else { "BLOCKED" },
                    reason,
                    "",
                )?;
                answer_dangling_calls(
                    tx,
                    instance,
                    epoch,
                    "Admission checkpoint recorded; no new work goal has been created",
                )?;
                append_context(
                    tx,
                    instance,
                    epoch,
                    "assistant",
                    &json!({"role":"assistant","content":reason}),
                    Some(&format!("work-{}-{decision}", if ask { "question" } else { "blocked" })),
                    &json!([]),
                )?;
            }
            _ => {
                answer_dangling_calls(
                    tx,
                    instance,
                    epoch,
                    "Use start_work to admit a new task, or answer an informational follow-up directly",
                )?;
                repair(tx, session, instance, "No new task has been admitted yet")?;
            }
        }
    } else {
        if params.get("completion").is_none()
            && params["entry"]["content"].as_str().is_none_or(|text| text.trim().is_empty())
        {
            answer_dangling_calls(tx, instance, epoch, "Answer the informational input or admit the requested work")?;
            return repair(tx, session, instance, "The input received no answer or work admission");
        }
        if let Some(completion) = params.get("completion") {
            let succeeded = completion["outcome"] == "success";
            answer_dangling_calls(
                tx,
                instance,
                epoch,
                if succeeded {
                    "Informational answer recorded without creating a new work goal"
                } else {
                    "Input remains blocked; no new work goal has been created"
                },
            )?;
            append_context(
                tx,
                instance,
                epoch,
                "assistant",
                &json!({"role":"assistant","content":completion["summary"]}),
                Some(&format!("work-{}-{decision}", if succeeded { "info" } else { "blocked" })),
                &json!([]),
            )?;
            tx.execute(
                "UPDATE instances SET phase='READY',revision=revision+1 WHERE id=?1 AND phase='COMPLETION_PENDING'",
                [instance],
            )
            .map_err(|e| format!("informational completion: {e}"))?;
            if !succeeded {
                set_state(
                    tx,
                    &(String::new(), instance.into(), epoch, String::new()),
                    "BLOCKED",
                    completion["summary"].as_str().unwrap_or("Input is blocked"),
                    "",
                )?;
                return Ok(());
            }
        }
        close(tx, instance, epoch)?;
        event(tx, session, "work_input_answered", instance, &json!({"admission_id":state["admission_id"]}))?;
    }
    Ok(())
}
