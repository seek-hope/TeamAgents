"""Information permissions: event audience, delivery push, and AgentView building.

Two layers (plan sections 2.2 rule 4 and 7):
  * ``audience`` — who may see an event at all (queries, observers, TUI).
  * ``push`` — who receives it as an inbox delivery (context injection).
The Leader sees task progress/results/directed messages in the log (section 1),
but private member contexts are never injected into the Leader.
"""

from __future__ import annotations

import json

from .models import (
    AgentView,
    EventKind,
    ObserverSpec,
    SharedEntry,
    TeamSpec,
    TurnRun,
)
from .storage import Store

#: Event kinds whose payloads may contain private content (message text, results).
CONTENT_KINDS = {
    EventKind.USER_MESSAGE,
    EventKind.MESSAGE,
    EventKind.TASK_COMPLETED,
    EventKind.TASK_FAILED,
    EventKind.TASK_BLOCKED,
    EventKind.SHARED_PUBLISHED,
}


def observer_matches(ob: ObserverSpec, kind: EventKind, actor_id: str,
                     subject_ids: set[str]) -> bool:
    if ob.subjects and not (subject_ids & set(ob.subjects)):
        return False
    return not ob.event_types or kind in ob.event_types


def observer_scope_for(spec: TeamSpec, recipient: str, kind: EventKind, actor_id: str,
                       payload: dict) -> str | None:
    """The payload scope this recipient sees as an observer (None = direct participant)."""
    subjects = _observer_subjects(kind, actor_id, payload)
    if recipient in subjects or recipient == actor_id:
        return None
    for ob in spec.observers:
        if ob.agent_id == recipient and observer_matches(ob, kind, actor_id, subjects):
            return ob.payload_scope
    return None


def _observer_subjects(kind: EventKind, actor_id: str, payload: dict) -> set[str]:
    """Which members an event is *about* (for observer subject matching)."""
    subs = {actor_id}
    for key in ("assignee", "requester", "target", "author", "agent_id"):
        if isinstance(v := payload.get(key), str):
            subs.add(v)
    return subs


def scope_payload(scope: str, kind: EventKind, payload: dict) -> dict:
    """Observer payload scoping: status < public_message < result."""
    if scope == "status":
        return {k: v for k, v in payload.items() if k in
                ("status", "task_id", "run_id", "agent_id", "assignee", "requester", "kind")}
    if scope == "public_message":
        if kind in (EventKind.MESSAGE, EventKind.USER_MESSAGE):
            return payload
        return {k: v for k, v in payload.items() if k in
                ("status", "task_id", "run_id", "agent_id", "assignee", "requester",
                 "summary", "description")}
    return payload  # result


def event_audience(spec: TeamSpec, kind: EventKind, actor_id: str,
                   payload: dict, targets: list[str] | None = None) -> list[str]:
    """Who may see this event; observers are added per their subscription."""
    all_members = {a.id for a in spec.agents}
    audience: set[str] = set()
    match kind:
        case EventKind.USER_MESSAGE:
            audience = {spec.leader_id}
        case EventKind.LEADER_REPLY:
            audience = {spec.leader_id}
        case EventKind.MESSAGE:
            audience = set(targets or []) | {spec.leader_id}
        case EventKind.TASK_CREATED | EventKind.TASK_STARTED | EventKind.TASK_COMPLETED \
                | EventKind.TASK_FAILED | EventKind.TASK_CANCELLED | EventKind.TASK_BLOCKED \
                | EventKind.TASK_READY:
            audience = {actor_id, spec.leader_id}
            audience |= {payload[k] for k in ("assignee", "requester") if payload.get(k)}
            audience &= all_members
        case EventKind.SHARED_PUBLISHED:
            space_id = payload.get("space_id")
            if space_id:
                sp = spec.space(space_id)
                audience = set(sp.readers) | set(sp.writers)
        case EventKind.APPROVAL_REQUESTED | EventKind.APPROVAL_DECIDED:
            audience = {spec.leader_id, payload.get("agent_id", actor_id)}
        case EventKind.RUN_PROGRESS:
            audience = {spec.leader_id, actor_id}
            if who := payload.get("requester"):
                audience.add(who)
        case EventKind.TOPOLOGY_PROPOSED | EventKind.TOPOLOGY_APPLIED | EventKind.TOPOLOGY_REJECTED:
            audience = {spec.leader_id, actor_id}
        case _:
            audience = {spec.leader_id}
    subjects = _observer_subjects(kind, actor_id, payload)
    for ob in spec.observers:
        if observer_matches(ob, kind, actor_id, subjects):
            audience.add(ob.agent_id)
    return sorted(a for a in audience if a in all_members)


def event_push(spec: TeamSpec, kind: EventKind, actor_id: str, payload: dict,
               targets: list[str] | None = None) -> list[str]:
    """Who receives this event as a context delivery."""
    all_members = {a.id for a in spec.agents}
    push: set[str] = set()
    match kind:
        case EventKind.USER_MESSAGE:
            goal_target = payload.get("to") or spec.leader_id
            push = {goal_target}
        case EventKind.LEADER_REPLY:
            push = set()
        case EventKind.MESSAGE:
            push = set(targets or [])
        case EventKind.TASK_CREATED | EventKind.TASK_STARTED | EventKind.TASK_COMPLETED \
                | EventKind.TASK_FAILED | EventKind.TASK_CANCELLED | EventKind.TASK_BLOCKED \
                | EventKind.TASK_READY:
            if kind is EventKind.TASK_READY and payload.get("assignee"):
                push.add(payload["assignee"])
            elif payload.get("requester"):
                push.add(payload["requester"])
            if kind in (EventKind.TASK_BLOCKED, EventKind.TASK_FAILED):
                push.add(spec.leader_id)
        case EventKind.TOPOLOGY_PROPOSED:
            push = {spec.leader_id}
        case EventKind.APPROVAL_REQUESTED:
            push = {spec.leader_id}
        case EventKind.RUN_PROGRESS:
            push = set()
        case EventKind.LIMIT_REACHED | EventKind.GOAL_DONE:
            push = {spec.leader_id}
        case _:
            push = set()
    subjects = _observer_subjects(kind, actor_id, payload)
    for ob in spec.observers:
        if ob.wake_policy == "on_event" and observer_matches(ob, kind, actor_id, subjects):
            push.add(ob.agent_id)
    return sorted(p for p in push if p in all_members and p != actor_id)


def build_agent_view(store: Store, spec: TeamSpec, session_id: str, agent_id: str,
                     run: TurnRun | None = None) -> AgentView:
    """What this member may see right now: assignments, inbox delta, shared delta."""
    assignment = [t for t in store.tasks_for_session(session_id) if t.assignee == agent_id]
    pending = store.pending_deliveries(session_id, agent_id)
    inbox: list[dict] = []
    delivery_ids: list[int] = []
    batch_max = 0
    for d in pending:
        payload = (json.loads(d["payload_override"]) if d["payload_override"]
                   else json.loads(d["payload_json"]))
        inbox.append({
            "event_id": d["event_id"],
            "kind": d["event_kind"],
            "from": d["event_actor"],
            "task_id": d["event_task_id"],
            "payload": payload,
        })
        delivery_ids.append(d["delivery_id"])
        batch_max = max(batch_max, d["batch_no"])

    readable_spaces = [s.id for s in spec.shared_spaces
                       if agent_id in s.readers or agent_id in s.writers]
    shared_delta: list[SharedEntry] = []
    for space_id in readable_spaces:
        cursor = store.shared_cursor(session_id, agent_id, space_id)
        shared_delta.extend(store.shared_entries(session_id, [space_id],
                                                 after_sequence=cursor, limit=50))
    capabilities = sorted(set(spec.agent(agent_id).tool_bindings))
    return AgentView(
        agent_id=agent_id,
        assignment=assignment,
        inbox_delta=inbox,
        permitted_shared_delta=shared_delta,
        relevant_topology={
            "revision": store.current_revision(session_id),
            "members": [{"id": a.id, "name": a.name, "role": a.role,
                         "runtime_kind": a.runtime_kind, "status": store.agent_status(session_id, a.id)}
                        for a in spec.agents],
            "can_send_to": sorted({a.id for a in spec.agents
                                   if spec.can_send(agent_id, a.id)}),
            "can_delegate_to": sorted({a.id for a in spec.agents
                                       if spec.can_delegate(agent_id, a.id)}),
            "shared_spaces": readable_spaces,
            "leader": spec.leader_id,
        },
        capabilities=capabilities,
        delivery_ids=delivery_ids,
        batch_no=batch_max,
    )
