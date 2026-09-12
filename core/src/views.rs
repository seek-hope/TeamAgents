//! Information permissions, ported from src/teamagents/views.py:
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
        || ob.event_types.iter().any(|t| {
            serde_json::to_value(kind).ok().and_then(|k| k.as_str().map(str::to_string)) == Some(t.clone())
        })
}

/// The payload scope this recipient sees as an observer (None = direct participant).
pub fn observer_scope_for(spec: &TeamSpec, recipient: &str, kind: EventKind, actor_id: &str, payload: &Json) -> Option<String> {
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
        Json::Object(map.iter().filter(|(k, _)| keys.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
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
        _ => payload.clone(), // result
    }
}

fn all_members(spec: &TeamSpec) -> HashSet<String> {
    spec.agents.iter().map(|a| a.id.clone()).collect()
}

pub fn event_audience(spec: &TeamSpec, kind: EventKind, actor_id: &str, payload: &Json, targets: Option<&[String]>) -> Vec<String> {
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
        EventKind::TaskCreated | EventKind::TaskStarted | EventKind::TaskCompleted
        | EventKind::TaskFailed | EventKind::TaskCancelled | EventKind::TaskBlocked | EventKind::TaskReady => {
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

pub fn event_push(spec: &TeamSpec, kind: EventKind, actor_id: &str, payload: &Json, targets: Option<&[String]>) -> Vec<String> {
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
        EventKind::TaskCreated | EventKind::TaskStarted | EventKind::TaskCompleted
        | EventKind::TaskFailed | EventKind::TaskCancelled | EventKind::TaskBlocked | EventKind::TaskReady => {
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
    fn user_message_pushes_to_leader() {
        let s = spec();
        let payload = serde_json::json!({"text": "hi", "to": "lead"});
        assert_eq!(event_push(&s, EventKind::UserMessage, "user", &payload, None), vec!["lead"]);
    }
}
