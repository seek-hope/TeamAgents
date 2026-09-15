//! T11–T13: topology changes go through the Leader and apply at a boundary.

mod support;

use serde_json::json;
use support::*;

fn add_worker_op(agent_id: &str) -> serde_json::Value {
    json!({
        "op": "add_agent",
        "agent": {"id": agent_id, "name": agent_id, "role": "worker", "runtime_kind": "deepagents",
                  "instructions": "work", "model_profile": "m", "tool_bindings": ["files"],
                  "skills": [], "workspace_policy": "shared"},
        "channels": [{"source": "leader", "targets": [agent_id], "mode": "task"}]
    })
}

fn spec() -> serde_json::Value {
    json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker")],
        "channels": [task_channel("leader", &["b"]), message_channel("b", &["leader"])],
        "shared_spaces": [{"id": "main", "readers": ["leader", "b"], "writers": ["leader", "b"]}],
    })
}

#[test]
fn t11_member_proposal_is_leader_decision() {
    isolated_state_home("t11");
    let core = core_with_spec("s1", spec());
    let barriers = barriers();
    let leader = scripted("leader", &json!([["inbox"], ["end"]]), barriers.clone());
    let b = scripted(
        "b",
        &json!([
            ["call", "propose_team_change", {"operations": [add_worker_op("worker")], "rationale": "need a helper"}],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id"}],
            ["end"],
        ]),
        barriers,
    );
    let h = harness_with(core.clone(), vec![("leader", leader), ("b", b)]);
    h.runtime.start();
    h.runtime.user_message("please start", false).unwrap();
    let assigned = submit(&core, "assign-1", "leader", "assign_task", json!({"assignee": "b", "description": "propose a helper"}));
    assert!(assigned.ok, "{:?}", assigned.error);
    assert!(h.runtime.settle(10));

    let state = h.runtime.state().unwrap();
    let kinds: Vec<String> = state["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert!(kinds.contains(&"topology_proposed".to_string()), "{kinds:?}");

    // approve the proposal as the Leader (patch_id path)
    let patches = core.call_in_session("state", json!({})).unwrap();
    let patch_id = patches["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("topology_proposed"))
        .and_then(|e| e.pointer("/payload/patch_id").and_then(|v| v.as_str()))
        .expect("proposal carries its patch id")
        .to_string();
    let applied = submit(&core, "apply-1", "leader", "apply_topology_patch", json!({"patch_id": patch_id}));
    assert!(applied.ok, "{:?}", applied.error);
    assert_eq!(applied.result["status"], "APPLIED", "{:?}", applied.result);

    let state = h.runtime.state().unwrap();
    let ids: Vec<String> = state["spec"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert!(ids.contains(&"worker".to_string()), "application adds the member: {ids:?}");
    assert!(state["revision"].as_i64().unwrap_or(0) >= 2);
    h.runtime.close();
}

#[test]
fn t12_conflicting_patches_never_partially_apply() {
    isolated_state_home("t12");
    let core = core_with_spec("s1", spec());
    let h = harness_with(core.clone(), vec![]);
    h.runtime.start();
    h.runtime.user_message("start", false).unwrap();

    let first = submit(&core, "patch-a", "leader", "apply_topology_patch",
                       json!({"operations": [add_worker_op("worker1")], "base_revision": 1}));
    assert!(first.ok, "{:?}", first.error);
    let stale = submit(&core, "patch-b", "leader", "apply_topology_patch",
                       json!({"operations": [add_worker_op("worker2")], "base_revision": 1}));
    assert!(!stale.ok, "a stale base revision must be refused");
    let message = stale.error.clone().unwrap_or_default();
    assert!(message.contains("stale") || message.contains("conflict"), "{message}");

    let state = h.runtime.state().unwrap();
    let ids: Vec<String> = state["spec"]["agents"].as_array().unwrap().iter()
        .filter_map(|a| a.get("id").and_then(|v| v.as_str()).map(str::to_string)).collect();
    assert!(ids.contains(&"worker1".to_string()));
    assert!(!ids.contains(&"worker2".to_string()), "the conflicting patch applied nothing");
    h.runtime.close();
}

#[test]
fn t13_removed_member_hands_tasks_to_leader_and_keeps_results() {
    isolated_state_home("t13");
    let core = core_with_spec("s1", spec());
    let barriers = barriers();
    let leader = scripted("leader", &json!([["end"]]), barriers.clone());
    let b = scripted(
        "b",
        &json!([
            ["call", "publish_shared", {"space_id": "main", "content": "partial result"}],
            ["sleep", 0.6],
            ["call", "complete_task", {"task_id": "$inbox0.payload.task_id"}],
            ["end"], ["end"],
        ]),
        barriers,
    );
    let h = harness_with(core.clone(), vec![("leader", leader), ("b", b)]);
    h.runtime.start();
    let first = submit(&core, "assign-first", "leader", "assign_task",
                       json!({"assignee": "b", "description": "publish partial work"}));
    assert!(first.ok, "{:?}", first.error);
    // wait until b is actually running, then queue a second task behind it
    assert!(wait_for(|| {
        h.runtime.state().ok()
            .map(|state| state["runs"].as_array().cloned().unwrap_or_default().iter().any(|r| {
                r.get("agent_id").and_then(|v| v.as_str()) == Some("b")
                    && r.get("status").and_then(|v| v.as_str()) == Some("RUNNING")
            }))
            .unwrap_or(false)
    }, 5000));
    let second = submit(&core, "assign-second", "leader", "assign_task",
                        json!({"assignee": "b", "description": "not started yet"}));
    assert!(second.ok, "{:?}", second.error);

    let removed = submit(&core, "remove-b", "leader", "apply_topology_patch",
                         json!({"operations": [{"op": "remove_agent", "agent_id": "b"}], "base_revision": 1}));
    assert!(removed.ok, "{:?}", removed.error);
    assert_eq!(removed.result["status"], "WAITING_BOUNDARY");
    assert!(h.runtime.settle(15));

    let state = h.runtime.state().unwrap();
    let ids: Vec<String> = state["spec"]["agents"].as_array().unwrap().iter()
        .filter_map(|a| a.get("id").and_then(|v| v.as_str()).map(str::to_string)).collect();
    assert!(!ids.contains(&"b".to_string()), "removal is applied at the boundary");
    let pending: Vec<&serde_json::Value> = state["tasks"].as_array().unwrap().iter()
        .filter(|t| matches!(t.get("status").and_then(|v| v.as_str()), Some("PENDING") | Some("BLOCKED")))
        .collect();
    assert!(!pending.is_empty(), "the queued task survives");
    assert!(pending.iter().all(|t| t.get("assignee").and_then(|v| v.as_str()) == Some("leader")),
            "unfinished work is handed back to the Leader: {pending:?}");
    let entries = core.call_in_session("shared_entries", json!({"space_ids": ["main"]})).unwrap();
    let contents: Vec<String> = entries["entries"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|e| e.get("content").and_then(|v| v.as_str()).map(str::to_string)).collect();
    assert!(contents.contains(&"partial result".to_string()), "results produced before removal are kept: {contents:?}");
    let kinds: Vec<String> = state["events"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string)).collect();
    assert!(kinds.contains(&"member_removed".to_string()), "removal is audited");
    h.runtime.close();
}
