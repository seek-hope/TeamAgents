//! Information permissions:
//! `audience` — who may see an event; `push` — who receives it as a delivery.

use crate::models::*;
use std::collections::HashSet;

pub const STATUS_KEYS: &[&str] = &["status", "task_id", "run_id", "agent_id", "assignee", "requester", "kind"];

fn observer_subjects(_kind: EventKind, actor_id: &str, payload: &Json) -> HashSet<String> {
    let mut subs = HashSet::from([actor_id.to_string()]);
    for key in ["assignee", "requester", "target", "author", "agent_id"] {
        if let Some(v) = payload.get(key).and_then(|v| v.as_str()) {
            subs.insert(v.to_string());
        }
    }
    subs
}

pub fn observer_matches(ob: &ObserverSpec, kind: EventKind, _actor_id: &str, subjects: &HashSet<String>) -> bool {
    if !ob.subjects.is_empty() && !ob.subjects.iter().any(|s| subjects.contains(s)) {
        return false;
    }
    ob.event_types.is_empty()
        || ob
            .event_types
            .iter()
            .any(|t| serde_json::to_value(kind).ok().and_then(|k| k.as_str().map(str::to_string)) == Some(t.clone()))
}

/// The payload scope this recipient sees as an observer (None = direct participant).
pub fn observer_scope_for(
    spec: &TeamSpec,
    recipient: &str,
    kind: EventKind,
    actor_id: &str,
    payload: &Json,
) -> Option<String> {
    let subjects = observer_subjects(kind, actor_id, payload);
    if subjects.contains(recipient) || recipient == actor_id {
        return None;
    }
    spec.observers
        .iter()
        .find(|ob| ob.agent_id == recipient && observer_matches(ob, kind, actor_id, &subjects))
        .map(|ob| ob.payload_scope.clone())
}

/// Observer payload scoping: status < public_message < result.
pub fn scope_payload(scope: &str, kind: EventKind, payload: &Json) -> Json {
    let Some(map) = payload.as_object() else { return payload.clone() };
    let keep = |keys: &[&str]| -> Json {
        Json::Object(
            map.iter().filter(|(k, _)| keys.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
        )
    };
    match scope {
        "status" => keep(STATUS_KEYS),
        "public_message" => {
            if matches!(kind, EventKind::Message | EventKind::UserMessage) {
                payload.clone()
            } else {
                keep(&["status", "task_id", "run_id", "agent_id", "assignee", "requester", "summary", "description"])
            }
        }
        "result" => payload.clone(),
        // fail closed: an unrecognized scope gets the minimal status tier
        _ => keep(STATUS_KEYS),
    }
}

fn all_members(spec: &TeamSpec) -> HashSet<String> {
    spec.agents.iter().map(|a| a.id.clone()).collect()
}

pub fn event_audience(
    spec: &TeamSpec,
    kind: EventKind,
    actor_id: &str,
    payload: &Json,
    targets: Option<&[String]>,
) -> Vec<String> {
    let members = all_members(spec);
    let mut audience: HashSet<String> = HashSet::new();
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match kind {
        EventKind::UserMessage | EventKind::LeaderReply => {
            audience.insert(spec.leader_id.clone());
        }
        EventKind::Message => {
            if let Some(ts) = targets {
                audience.extend(ts.iter().cloned());
            }
            audience.insert(spec.leader_id.clone());
        }
        EventKind::TaskCreated
        | EventKind::TaskStarted
        | EventKind::TaskCompleted
        | EventKind::TaskFailed
        | EventKind::TaskCancelled
        | EventKind::TaskBlocked
        | EventKind::TaskReady => {
            audience.insert(actor_id.to_string());
            audience.insert(spec.leader_id.clone());
            for k in ["assignee", "requester"] {
                if let Some(v) = get(k) {
                    audience.insert(v);
                }
            }
            audience = audience.intersection(&members).cloned().collect();
        }
        EventKind::SharedPublished => {
            if let Some(sid) = get("space_id") {
                if let Some(sp) = spec.space(&sid) {
                    audience.extend(sp.readers.iter().cloned());
                    audience.extend(sp.writers.iter().cloned());
                }
            }
        }
        EventKind::ApprovalRequested | EventKind::ApprovalDecided => {
            audience.insert(spec.leader_id.clone());
            audience.insert(get("agent_id").unwrap_or_else(|| actor_id.to_string()));
        }
        EventKind::RunProgress => {
            audience.insert(spec.leader_id.clone());
            audience.insert(actor_id.to_string());
            if let Some(r) = get("requester") {
                audience.insert(r);
            }
        }
        EventKind::TopologyProposed | EventKind::TopologyApplied | EventKind::TopologyRejected => {
            audience.insert(spec.leader_id.clone());
            audience.insert(actor_id.to_string());
        }
        _ => {
            audience.insert(spec.leader_id.clone());
        }
    }
    let subjects = observer_subjects(kind, actor_id, payload);
    for ob in &spec.observers {
        if observer_matches(ob, kind, actor_id, &subjects) {
            audience.insert(ob.agent_id.clone());
        }
    }
    let mut out: Vec<String> = audience.into_iter().filter(|a| members.contains(a)).collect();
    out.sort();
    out
}

pub fn event_push(
    spec: &TeamSpec,
    kind: EventKind,
    actor_id: &str,
    payload: &Json,
    targets: Option<&[String]>,
) -> Vec<String> {
    let members = all_members(spec);
    let mut push: HashSet<String> = HashSet::new();
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match kind {
        EventKind::UserMessage => {
            push.insert(get("to").unwrap_or_else(|| spec.leader_id.clone()));
        }
        EventKind::Message => {
            if let Some(ts) = targets {
                push.extend(ts.iter().cloned());
            }
        }
        EventKind::TaskCreated
        | EventKind::TaskStarted
        | EventKind::TaskCompleted
        | EventKind::TaskFailed
        | EventKind::TaskCancelled
        | EventKind::TaskBlocked
        | EventKind::TaskReady => {
            if kind == EventKind::TaskReady {
                if let Some(a) = get("assignee") {
                    push.insert(a);
                }
            } else if let Some(r) = get("requester") {
                push.insert(r);
            }
            if matches!(kind, EventKind::TaskBlocked | EventKind::TaskFailed) {
                push.insert(spec.leader_id.clone());
            }
        }
        EventKind::TopologyProposed | EventKind::ApprovalRequested => {
            push.insert(spec.leader_id.clone());
        }
        EventKind::LimitReached | EventKind::GoalDone => {
            push.insert(spec.leader_id.clone());
        }
        _ => {}
    }
    let subjects = observer_subjects(kind, actor_id, payload);
    for ob in &spec.observers {
        if ob.wake_policy == "on_event" && observer_matches(ob, kind, actor_id, &subjects) {
            push.insert(ob.agent_id.clone());
        }
    }
    let mut out: Vec<String> = push.into_iter().filter(|p| members.contains(p) && p != actor_id).collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TeamSpec {
        serde_json::from_value(serde_json::json!({
            "leader_id": "lead",
            "agents": [
                {"id": "lead", "name": "L", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
                {"id": "b", "name": "B", "role": "worker", "runtime_kind": "codex", "model_profile": "m"},
                {"id": "obs", "name": "O", "role": "observer", "runtime_kind": "codex", "model_profile": "m"}
            ],
            "channels": [{"source": "lead", "targets": ["b"], "mode": "message"}],
            "observers": [{"agent_id": "obs", "subjects": ["b"], "payload_scope": "status", "wake_policy": "on_event"}]
        }))
        .unwrap()
    }

    #[test]
    fn task_completed_reaches_requester_only() {
        let s = spec();
        let payload = serde_json::json!({"task_id": "t1", "assignee": "b", "requester": "lead", "summary": "done"});
        let push = event_push(&s, EventKind::TaskCompleted, "b", &payload, None);
        assert_eq!(push, vec!["lead", "obs"]); // on_event observer also wakes
        let aud = event_audience(&s, EventKind::TaskCompleted, "b", &payload, None);
        assert_eq!(aud, vec!["b", "lead", "obs"]); // observer subscribed to b
                                                   // observer payload is scoped to status keys
        let scope = observer_scope_for(&s, "obs", EventKind::TaskCompleted, "b", &payload);
        assert_eq!(scope.as_deref(), Some("status"));
        let trimmed = scope_payload("status", EventKind::TaskCompleted, &payload);
        assert!(trimmed.get("summary").is_none());
        assert!(trimmed.get("task_id").is_some());
    }

    #[test]
    fn unknown_observer_scope_is_rejected_and_fails_closed() {
        let mut s = spec();
        s.observers[0].payload_scope = "bogus-typo".into();
        assert!(s.validate().unwrap_err().contains("payload_scope"));
        s.observers[0].payload_scope = "result".into();
        s.observers[0].wake_policy = "sometimes".into();
        assert!(s.validate().unwrap_err().contains("wake_policy"));

        // even if a bad scope slipped past validation, trimming fails closed
        let payload = serde_json::json!({"task_id": "t1", "summary": "TOP-SECRET-SUMMARY"});
        let trimmed = scope_payload("bogus-typo", EventKind::TaskCompleted, &payload);
        assert!(trimmed.get("summary").is_none());
        assert!(trimmed.get("task_id").is_some());
        // the real tiers behave as before
        assert!(scope_payload("result", EventKind::TaskCompleted, &payload).get("summary").is_some());
        assert!(scope_payload("status", EventKind::TaskCompleted, &payload).get("summary").is_none());
    }

    #[test]
    fn user_message_pushes_to_leader() {
        let s = spec();
        let payload = serde_json::json!({"text": "hi", "to": "lead"});
        assert_eq!(event_push(&s, EventKind::UserMessage, "user", &payload, None), vec!["lead"]);
    }
}

// -- AgentView -------------------------------------------------------------------

use crate::storage::Store;

/// What one member may see at a delivery boundary (plan §5.2).
pub fn build_agent_view(store: &Store, spec: &TeamSpec, session_id: &str, agent_id: &str) -> Json {
    let all_tasks = store.tasks_for_session(session_id, &[]).unwrap_or_default();
    let assignment: Vec<&Task> = all_tasks.iter().filter(|t| t.assignee == agent_id).collect();
    let pending = store.pending_deliveries_joined(session_id, agent_id).unwrap_or_default();
    let mut inbox = vec![];
    let mut delivery_ids: Vec<i64> = vec![];
    let mut batch_max = 0_i64;
    for d in &pending {
        let payload = d["payload_override"]
            .as_str()
            .and_then(|o| serde_json::from_str(o).ok())
            .or_else(|| serde_json::from_str(d["payload_json"].as_str().unwrap_or("null")).ok())
            .unwrap_or(Json::Null);
        inbox.push(serde_json::json!({
            "delivery_id": d["delivery_id"],
            "event_id": d["event_id"],
            "kind": d["event_kind"],
            "from": d["event_actor"],
            "task_id": d["event_task_id"],
            "payload": payload,
        }));
        if let Some(id) = d["delivery_id"].as_i64() {
            delivery_ids.push(id);
        }
        batch_max = batch_max.max(d["batch_no"].as_i64().unwrap_or(0));
    }
    let readable: Vec<String> = spec
        .shared_spaces
        .iter()
        .filter(|s| s.readers.iter().any(|r| r == agent_id) || s.writers.iter().any(|w| w == agent_id))
        .map(|s| s.id.clone())
        .collect();
    let mut shared_delta: Vec<SharedEntry> = vec![];
    for sid in &readable {
        let cursor = store.shared_cursor(session_id, agent_id, sid).unwrap_or(0);
        shared_delta.extend(store.shared_entries(session_id, &[sid.clone()], cursor, 50).unwrap_or_default());
    }
    let capabilities: Vec<String> = spec.agent(agent_id).map(|a| a.tool_bindings.clone()).unwrap_or_default();
    let members: Vec<Json> = spec
        .agents
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id, "name": a.name, "role": a.role,
                "runtime_kind": a.runtime_kind,
                // what a member can actually execute: the Leader delegates
                // against this, so an empty list is visible instead of silent
                "tools": a.tool_bindings,
                "status": store.agent_status(session_id, &a.id).ok().flatten()
                    .map(|s| serde_json::to_value(s).unwrap_or(Json::Null)).unwrap_or(Json::Null),
            })
        })
        .collect();
    let mut can_send_to: Vec<String> =
        spec.agents.iter().filter(|a| spec.can_send(agent_id, &a.id)).map(|a| a.id.clone()).collect();
    can_send_to.sort();
    let mut can_delegate_to: Vec<String> =
        spec.agents.iter().filter(|a| spec.can_delegate(agent_id, &a.id)).map(|a| a.id.clone()).collect();
    can_delegate_to.sort();
    serde_json::json!({
        "agent_id": agent_id,
        "assignment": assignment,
        "inbox_delta": inbox,
        "permitted_shared_delta": shared_delta,
        "relevant_topology": {
            "revision": store.current_revision(session_id).unwrap_or(0),
            "members": members,
            "can_send_to": can_send_to,
            "can_delegate_to": can_delegate_to,
            "shared_spaces": readable,
            "leader": spec.leader_id,
        },
        "capabilities": capabilities,
        "delivery_ids": delivery_ids,
        "batch_no": batch_max,
    })
}
