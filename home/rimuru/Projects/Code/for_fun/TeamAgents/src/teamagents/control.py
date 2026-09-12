"""Team control flow: ingest -> validate -> reduce -> schedule -> persist.

Every team action enters here exactly once; the business transaction (receipt,
events, task/shared-space changes, queued execution intents) commits together.
Action ids are stable (`run_id:tool_call_id`), repeats return the original receipt.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import Any

from . import views
from .models import (
    ActionKind,
    AgentSpec,
    AgentStatus,
    ApprovalStatus,
    ChannelSpec,
    EventKind,
    Limits,
    PatchStatus,
    Receipt,
    SessionStatus,
    SharedEntry,
    Task,
    TaskStatus,
    TeamAction,
    TeamEvent,
    TeamSpec,
    TopologyPatch,
    TurnRun,
    TurnStatus,
    TURN_TERMINAL_STATUSES,
    UserConfig,
    new_id,
    now,
)
from .storage import Store

#: capabilities the runtime provides natively; they need no user-config entry
BUILTIN_TOOL_BINDINGS = {"files", "shell", "web"}


@dataclass
class EventDraft:
    kind: EventKind
    payload: dict[str, Any] = field(default_factory=dict)
    task_id: str | None = None
    targets: list[str] | None = None
    actor_id: str | None = None
    audience: list[str] | None = None
    push: list[str] | None = None


@dataclass
class Reduction:
    events: list[EventDraft]
    receipt: Receipt


class Control:
    """The single serialized entry point for team transactions per session."""

    def __init__(self, store: Store, session_id: str, catalog: UserConfig | None = None):
        self.store = store
        self.session_id = session_id
        self.catalog = catalog or UserConfig()
        self.last_schedule_error: str | None = None
        #: (run_id, inbox items) recorded when a running member receives mid-turn input
        self.mid_turn_pushes: list[tuple[str, list[dict]]] = []

    # ------------------------------------------------------------------ submit

    def submit(self, action: TeamAction) -> Receipt:
        try:
            return self._submit_once(action)
        except Exception as e:
            # malformed model input is a readable refusal, never a crash. The
            # failed attempt's transaction rolled back every write it made; the
            # failure receipt then commits in a clean transaction. Retrying the
            # same action id replays that receipt; a new action id may retry
            # against the clean state.
            receipt = Receipt.failure(action, f"{type(e).__name__}: {e}")
            with self.store.tx():
                prior = self.store.get_action_receipt(action.action_id)
                if prior is not None:  # a concurrent writer already recorded it
                    return prior
                self.store.record_action(
                    action.action_id, self.session_id, action.actor_id, action.run_id,
                    action.kind, self._payload_hash(action), receipt,
                )
            return receipt

    def _submit_once(self, action: TeamAction) -> Receipt:
        with self.store.tx():
            prior = self.store.get_action_receipt(action.action_id)
            if prior is not None:
                return prior
            spec = self.store.load_team_spec(self.session_id)
            error = self._validate(action, spec)
            if error is not None:
                receipt = Receipt.failure(action, error)
                self.store.record_action(
                    action.action_id, self.session_id, action.actor_id, action.run_id,
                    action.kind, self._payload_hash(action), receipt,
                )
                return receipt
            reduction = self._reduce(action, spec)
            self._persist_events(action, spec, reduction.events)
            self._schedule(spec)
            self.store.record_action(
                action.action_id, self.session_id, action.actor_id, action.run_id,
                action.kind, self._payload_hash(action), reduction.receipt,
            )
            return reduction.receipt

    @staticmethod
    def _derived_task_id(action: TeamAction) -> str:
        """Deterministic per action id, unique even for several tasks in one run."""
        if explicit := action.payload.get("task_id"):
            return str(explicit)
        digest = hashlib.sha256(action.action_id.encode()).hexdigest()[:12]
        return f"task_{digest}"

    @staticmethod
    def _payload_hash(action: TeamAction) -> str:
        blob = json.dumps(action.payload, sort_keys=True, ensure_ascii=False)
        return hashlib.sha256(blob.encode()).hexdigest()[:32]

    # ------------------------------------------------------------ public API

    def schedule(self) -> None:
        """Re-run scheduling against the committed state (called after turn ends)."""
        with self.store.tx():
            self._schedule(self.store.load_team_spec(self.session_id))

    def emit(self, drafts: list[EventDraft], actor_id: str = "system") -> None:
        """Persist runtime-originated events (run/task lifecycle) and schedule."""
        with self.store.tx():
            spec = self.store.load_team_spec(self.session_id)
            action = TeamAction(action_id=new_id("sys"), session_id=self.session_id,
                                actor_id=actor_id, kind=ActionKind.SEND_MESSAGE, payload={})
            self._persist_events(action, spec, drafts)
            self._schedule(spec)

    # -------------------------------------------------------------- validation

    def _validate(self, action: TeamAction, spec: TeamSpec) -> str | None:
        kind = action.kind
        member_ids = {a.id for a in spec.agents}
        actor = action.actor_id

        if kind is ActionKind.USER_MESSAGE or kind is ActionKind.USER_SUPPLEMENT:
            if actor != "user":
                return "only the local user can submit user input"
            if not str(action.payload.get("text", "")).strip():
                return "user input text must not be empty"
            return None

        # actor identity is runtime-injected; the user actor may only drive
        # control-plane actions, never member actions
        user_kinds = {ActionKind.CANCEL_TASK, ActionKind.CANCEL_RUN,
                      ActionKind.APPROVAL_DECISION, ActionKind.SET_PERMISSION_MODE,
                      ActionKind.PAUSE_SESSION}
        if actor == "user":
            if kind not in user_kinds:
                return f"the local user cannot submit {kind}"
        elif actor not in member_ids:
            return f"actor {actor!r} is not a team member"
        if actor in member_ids and action.run_id is not None:
            run = self.store.get_run(action.run_id)
            if run is None:
                return f"unknown run {action.run_id!r}"
            if run.agent_id != actor:
                return "run does not belong to the acting member"

        match kind:
            case ActionKind.SEND_MESSAGE:
                target = action.payload.get("target")
                if target == "*":
                    if not any(ch.source == actor and ch.mode.value == "broadcast"
                               for ch in spec.channels):
                        return f"{actor!r} has no broadcast channel"
                    return None
                if target not in member_ids:
                    return f"unknown message target {target!r}"
                if not spec.can_send(actor, target):
                    return f"{actor!r} is not allowed to message {target!r}"
            case ActionKind.ASSIGN_TASK:
                assignee = action.payload.get("assignee")
                if assignee not in member_ids:
                    return f"unknown assignee {assignee!r}"
                if not spec.can_delegate(actor, assignee):
                    return f"{actor!r} is not allowed to assign tasks to {assignee!r}"
                if (spec.agent(assignee).runtime_kind.value == "codex"
                        and actor != spec.leader_id):
                    return "codex members can only be delegated to by the Leader"
                if not str(action.payload.get("description", "")).strip():
                    return "task description must not be empty"
                deps = action.payload.get("dependencies") or []
                for dep in deps:
                    dep_task = self.store.get_task(dep)
                    if dep_task is None:
                        return f"unknown dependency task {dep!r}"
                new_task_id = self._derived_task_id(action)
                if self._dependency_cycle(new_task_id, deps):
                    return "task dependencies would form a cycle"
            case ActionKind.COMPLETE_TASK:
                task = self.store.get_task(action.payload.get("task_id", ""))
                if task is None:
                    return f"unknown task {action.payload.get('task_id')!r}"
                if task.assignee != actor:
                    return "only the current assignee can complete a task"
                if task.status not in (TaskStatus.PENDING, TaskStatus.RUNNING):
                    return f"task is {task.status}, cannot complete"
            case ActionKind.WAIT_FOR_TASKS:
                if action.run_id is None:
                    return "wait_for_tasks requires an active run"
                for tid in action.payload.get("task_ids") or []:
                    if self.store.get_task(tid) is None:
                        return f"unknown task {tid!r}"
            case ActionKind.PUBLISH_SHARED:
                space_id = action.payload.get("space_id")
                try:
                    space = spec.space(space_id)
                except ValueError as e:
                    return str(e)
                if actor not in space.writers:
                    return f"{actor!r} has no write access to shared space {space_id!r}"
                if not (action.payload.get("content") or action.payload.get("ref")):
                    return "shared entry needs content or a ref"
            case ActionKind.READ_SHARED | ActionKind.LIST_SHARED:
                space_id = action.payload.get("space_id")
                if space_id is not None:
                    try:
                        space = spec.space(space_id)
                    except ValueError as e:
                        return str(e)
                    if actor not in space.readers and actor not in space.writers:
                        return f"{actor!r} has no read access to shared space {space_id!r}"
            case ActionKind.REQUEST_HELP:
                if not str(action.payload.get("message", "")).strip():
                    return "help request must include a message"
            case ActionKind.PROPOSE_TEAM_CHANGE:
                ops = action.payload.get("operations")
                if not isinstance(ops, list) or not ops:
                    return "proposal needs a non-empty operations list"
            case ActionKind.APPLY_TOPOLOGY_PATCH:
                if actor != spec.leader_id:
                    return "only the Leader can apply topology patches"
                patch_id = action.payload.get("patch_id")
                if patch_id:
                    patch = self.store.get_patch(patch_id)
                    if patch is None:
                        return f"unknown patch {patch_id!r}"
                    if patch.status not in (PatchStatus.PROPOSED, PatchStatus.ACCEPTED):
                        return f"patch is {patch.status}, cannot decide"
                    return None
                ops = action.payload.get("operations")
                if not isinstance(ops, list) or not ops:
                    return "patch needs a non-empty operations list"
                base = action.payload.get("base_revision")
                if base != self.store.current_revision(self.session_id):
                    return (f"patch base_revision {base} is stale; "
                            f"current is {self.store.current_revision(self.session_id)}")
            case ActionKind.SIGNAL_DONE:
                if actor != spec.leader_id:
                    return "only the Leader can signal goal completion"
                if action.run_id is None:
                    return "signal_done requires an active run"
            case ActionKind.CANCEL_TASK:
                if actor not in (spec.leader_id, "user"):
                    return "only the Leader or the user can cancel tasks"
                if self.store.get_task(action.payload.get("task_id", "")) is None:
                    return f"unknown task {action.payload.get('task_id')!r}"
            case ActionKind.CANCEL_RUN:
                if actor not in (spec.leader_id, "user"):
                    return "only the Leader or the user can cancel runs"
                if self.store.get_run(action.payload.get("run_id", "")) is None:
                    return f"unknown run {action.payload.get('run_id')!r}"
            case ActionKind.APPROVAL_DECISION:
                if actor != "user":
                    return "only the local user can decide approvals"
                req = self.store.get_approval(action.payload.get("approval_id", ""))
                if req is None:
                    return f"unknown approval {action.payload.get('approval_id')!r}"
                if req.status is not ApprovalStatus.PENDING:
                    return f"approval is already {req.status}"
                if action.payload.get("decision") not in ("once", "session", "deny"):
                    return "decision must be one of: once, session, deny"
            case ActionKind.SET_PERMISSION_MODE:
                if actor != "user":
                    return "only the local user can change the permission mode"
                if action.payload.get("mode") not in ("approved_scope", "full_auto"):
                    return "mode must be approved_scope or full_auto"
            case ActionKind.PAUSE_SESSION:
                if actor != "user":
                    return "only the local user can pause the session"
            case _:
                return f"unsupported action kind {kind}"
        return None

    def _dependency_cycle(self, new_task_id: str, deps: list[str]) -> bool:
        """Adding new_task_id -> deps edges must not create a cycle."""
        seen: set[str] = set()
        stack = list(deps)
        while stack:
            tid = stack.pop()
            if tid == new_task_id:
                return True
            if tid in seen:
                continue
            seen.add(tid)
            t = self.store.get_task(tid)
            if t is not None:
                stack.extend(t.dependencies)
        return False

    # ------------------------------------------------------------------ reduce

    def _reduce(self, action: TeamAction, spec: TeamSpec) -> Reduction:
        kind = action.kind
        actor = action.actor_id
        p = action.payload

        match kind:
            case ActionKind.USER_MESSAGE | ActionKind.USER_SUPPLEMENT:
                # user input resumes a paused session (plan section 9.3)
                session = self.store.get_session(self.session_id)
                if session is not None and SessionStatus(session["status"]) is SessionStatus.PAUSED:
                    self.store.set_session_status(self.session_id, SessionStatus.ACTIVE)
                goal_id = self._ensure_goal(spec)
                text = str(p["text"])
                return Reduction(
                    events=[EventDraft(
                        kind=EventKind.USER_MESSAGE,
                        payload={"text": text, "goal_id": goal_id,
                                 "supplement": kind is ActionKind.USER_SUPPLEMENT,
                                 "to": spec.leader_id},
                        actor_id="user",
                    )],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"received": True, "goal_id": goal_id}),
                )

            case ActionKind.SEND_MESSAGE:
                target = p.get("target")
                targets = ([a.id for a in spec.agents if a.id != actor] if target == "*"
                           else [target])
                targets = [t for t in targets if t != actor and spec.can_send(actor, t)]
                text = str(p.get("text", ""))
                return Reduction(
                    events=[EventDraft(kind=EventKind.MESSAGE,
                                       payload={"text": text, "from": actor, "target": t},
                                       targets=[t]) for t in targets],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"delivered_to": targets}),
                )

            case ActionKind.ASSIGN_TASK:
                deps = list(p.get("dependencies") or [])
                task = Task(
                    task_id=self._derived_task_id(action),
                    parent_task_id=p.get("parent_task_id"),
                    goal_id=self._current_goal_id(),
                    requester=actor,
                    assignee=p["assignee"],
                    description=str(p["description"]),
                    acceptance=str(p.get("acceptance", "")),
                    dependencies=deps,
                )
                self.store.insert_task(self.session_id, task)
                return Reduction(
                    events=[EventDraft(kind=EventKind.TASK_CREATED,
                                       payload={"task_id": task.task_id, "assignee": task.assignee,
                                                "requester": task.requester,
                                                "description": task.description,
                                                "acceptance": task.acceptance,
                                                "dependencies": deps,
                                                "status": "PENDING"},
                                       task_id=task.task_id)],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"task_id": task.task_id}),
                )

            case ActionKind.COMPLETE_TASK:
                task_id = p["task_id"]
                result_refs = list(p.get("result_refs") or [])
                summary = str(p.get("summary", ""))
                self.store.record_completion_request(
                    action.run_id or "", task_id, result_refs, summary)
                return Reduction(
                    events=[],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"recorded": True, "task_id": task_id,
                                            "applies_at": "turn_end"}),
                )

            case ActionKind.WAIT_FOR_TASKS:
                task_ids = list(p.get("task_ids") or [])
                pending = [t for t in task_ids
                           if (task := self.store.get_task(t)) is not None
                           and task.status not in (TaskStatus.SUCCEEDED, TaskStatus.FAILED,
                                                   TaskStatus.CANCELLED)]
                if pending:
                    run = self.store.get_run(action.run_id or "")
                    if run is not None:
                        self.store.update_run_status_where(
                            run.run_id, TurnStatus.RUNNING, TurnStatus.WAITING_TASK)
                        self.store.deliver_wait_registration(run.run_id, pending)
                    return Reduction(
                        events=[EventDraft(kind=EventKind.RUN_WAITING,
                                           payload={"run_id": action.run_id, "agent_id": actor,
                                                    "waiting_on": pending})],
                        receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                        result={"waiting": True, "task_ids": pending}),
                    )
                results = {tid: self._task_result(tid) for tid in task_ids}
                return Reduction(
                    events=[],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"waiting": False, "results": results}),
                )

            case ActionKind.PUBLISH_SHARED:
                entry = SharedEntry(
                    entry_id=new_id("share"),
                    space_id=p["space_id"],
                    author=actor,
                    kind=str(p.get("kind", "note")),
                    content=str(p.get("content", "")),
                    ref=p.get("ref"),
                    supersedes=p.get("supersedes"),
                )
                seq = self.store.add_shared_entry(entry, self.session_id)
                return Reduction(
                    events=[EventDraft(kind=EventKind.SHARED_PUBLISHED,
                                       payload={"entry_id": entry.entry_id,
                                                "space_id": entry.space_id, "author": actor,
                                                "kind": entry.kind,
                                                "summary": entry.content[:200] or entry.ref,
                                                "sequence": seq})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"entry_id": entry.entry_id, "sequence": seq}),
                )

            case ActionKind.READ_SHARED:
                return self._read_shared(action, spec, advance=True)
            case ActionKind.LIST_SHARED:
                spaces = [s for s in spec.shared_spaces
                          if actor in s.readers or actor in s.writers]
                infos = []
                for s in spaces:
                    last = self.store.shared_entries(self.session_id, [s.id], limit=1)
                    entries = self.store.shared_entries(self.session_id, [s.id], limit=1000)
                    infos.append({"space_id": s.id, "entries": len(entries),
                                  "last_sequence": entries[-1].sequence if entries else 0,
                                  "readable": True, "writable": actor in s.writers})
                return Reduction(
                    events=[],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"spaces": infos}),
                )

            case ActionKind.REQUEST_HELP:
                task_id = p.get("task_id")
                return Reduction(
                    events=[EventDraft(kind=EventKind.MESSAGE,
                                       payload={"text": str(p["message"]), "from": actor,
                                                "target": spec.leader_id, "help": True,
                                                "task_id": task_id},
                                       targets=[spec.leader_id], task_id=task_id)],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"sent": True, "to": spec.leader_id}),
                )

            case ActionKind.PROPOSE_TEAM_CHANGE:
                patch = TopologyPatch(
                    patch_id=new_id("patch"),
                    base_revision=self.store.current_revision(self.session_id),
                    proposer=actor,
                    operations=list(p["operations"]),
                )
                self.store.insert_patch(patch, self.session_id)
                return Reduction(
                    events=[EventDraft(kind=EventKind.TOPOLOGY_PROPOSED,
                                       payload={"patch_id": patch.patch_id, "proposer": actor,
                                                "base_revision": patch.base_revision,
                                                "operations": patch.operations,
                                                "rationale": p.get("rationale", "")})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"patch_id": patch.patch_id, "status": "PROPOSED"}),
                )

            case ActionKind.APPLY_TOPOLOGY_PATCH:
                return self._apply_patch_action(action, spec)

            case ActionKind.SIGNAL_DONE:
                blockers = self._completion_blockers(spec, current_run=action.run_id)
                if blockers:
                    return Reduction(
                        events=[],
                        receipt=Receipt(action_id=action.action_id, ok=False, kind=kind,
                                        error="goal not yet complete",
                                        result={"blockers": blockers}),
                    )
                self.store.record_completion_request(action.run_id or "", "", [], str(p.get("summary", "")))
                return Reduction(
                    events=[],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"accepted": True,
                                            "note": "completion commits when the turn ends",
                                            "summary": p.get("summary", "")}),
                )

            case ActionKind.CANCEL_TASK:
                return self._cancel_task(action, spec)

            case ActionKind.CANCEL_RUN:
                run = self.store.get_run(p["run_id"])
                self.store.set_run_cancel_requested(run.run_id)
                return Reduction(
                    events=[EventDraft(kind=EventKind.RUN_CANCELLED,
                                       payload={"run_id": run.run_id, "agent_id": run.agent_id,
                                                "status": "CANCEL_REQUESTED"})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"run_id": run.run_id, "status": "cancel_requested"}),
                )

            case ActionKind.APPROVAL_DECISION:
                req = self.store.get_approval(p["approval_id"])
                decision = p["decision"]
                status = {"once": ApprovalStatus.APPROVED_ONCE,
                          "session": ApprovalStatus.APPROVED_SESSION,
                          "deny": ApprovalStatus.DENIED}[decision]
                self.store.decide_approval(req.approval_id, status)
                if status is ApprovalStatus.APPROVED_SESSION:
                    self.store.cache_session_approval(self.session_id, req.operation_hash,
                                                      req.requested_scope)
                self._wake_approval_run(req.run_id)
                return Reduction(
                    events=[EventDraft(kind=EventKind.APPROVAL_DECIDED,
                                       payload={"approval_id": req.approval_id,
                                                "agent_id": req.agent_id,
                                                "status": status.value})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"approval_id": req.approval_id, "status": status.value}),
                )

            case ActionKind.SET_PERMISSION_MODE:
                self.store.set_permission_mode(self.session_id, p["mode"])
                return Reduction(
                    events=[EventDraft(kind=EventKind.SESSION_STATUS,
                                       payload={"permission_mode": p["mode"]})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"mode": p["mode"]}),
                )

            case ActionKind.PAUSE_SESSION:
                self.store.set_session_status(self.session_id, SessionStatus.PAUSED)
                return Reduction(
                    events=[EventDraft(kind=EventKind.SESSION_STATUS,
                                       payload={"status": "PAUSED"})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=kind,
                                    result={"status": "PAUSED"}),
                )

        return Reduction(
            events=[],
            receipt=Receipt.failure(action, f"unsupported action kind {kind}"),
        )

    # ------------------------------------------------------------- reductions

    def _read_shared(self, action: TeamAction, spec: TeamSpec,
                     advance: bool) -> Reduction:
        actor = action.actor_id
        p = action.payload
        space_id = p.get("space_id")
        if space_id is None:
            spaces = [s.id for s in spec.shared_spaces
                      if actor in s.readers or actor in s.writers]
        else:
            spaces = [space_id]
        after = p.get("after_sequence")
        if after is None:
            after = min((self.store.shared_cursor(self.session_id, actor, s) for s in spaces),
                        default=0)
        entries = self.store.shared_entries(self.session_id, spaces,
                                            after_sequence=int(after),
                                            limit=int(p.get("limit", 50)))
        if advance and entries:
            for s in spaces:
                seq = max((e.sequence for e in entries if e.space_id == s), default=0)
                if seq:
                    self.store.advance_shared_cursor(self.session_id, actor, s, seq)
        return Reduction(
            events=[],
            receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                            result={"entries": [e.model_dump(mode="json") for e in entries],
                                    "next_sequence": entries[-1].sequence if entries else after}),
        )

    def _apply_patch_action(self, action: TeamAction, spec: TeamSpec) -> Reduction:
        p = action.payload
        patch_id = p.get("patch_id")
        if patch_id:
            patch = self.store.get_patch(patch_id)
            if p.get("reject"):
                self.store.set_patch_status(patch.patch_id, PatchStatus.REJECTED)
                return Reduction(
                    events=[EventDraft(kind=EventKind.TOPOLOGY_REJECTED,
                                       payload={"patch_id": patch.patch_id,
                                                "proposer": patch.proposer})],
                    receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                                    result={"patch_id": patch.patch_id, "status": "REJECTED"}),
                )
            operations = p.get("operations") or patch.operations
            decided_by = action.actor_id
            new_spec, error = self._apply_operations(spec, operations)
            if error:
                return Reduction([], Receipt.failure(action, error))
            revision = self.store.save_team_spec(self.session_id, new_spec)
            self.store.set_patch_status(patch.patch_id, PatchStatus.APPLIED)
            self._sync_members(new_spec, spec)
            return Reduction(
                events=[EventDraft(kind=EventKind.TOPOLOGY_APPLIED,
                                   payload={"patch_id": patch.patch_id, "revision": revision,
                                            "decided_by": decided_by,
                                            "operations": operations})],
                receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                                result={"patch_id": patch.patch_id, "revision": revision,
                                        "status": "APPLIED"}),
            )

        operations = list(p["operations"])
        new_spec, error = self._apply_operations(spec, operations)
        if error:
            return Reduction([], Receipt.failure(action, error))
        affected = self._affected_agents(spec, new_spec, operations)
        patch = TopologyPatch(
            patch_id=new_id("patch"),
            base_revision=p["base_revision"],
            proposer=action.actor_id,
            decided_by=action.actor_id,
            operations=operations,
            affected_agents=affected,
        )
        waiting = [a for a in affected if self._agent_has_live_run(a)]
        operations_json = operations
        if waiting:
            patch.status = PatchStatus.WAITING_BOUNDARY
            self.store.insert_patch(patch, self.session_id)
            for a in affected:
                if self._agent_has_live_run(a):
                    self.store.set_agent_status(self.session_id, a, AgentStatus.DRAINING)
            return Reduction(
                events=[EventDraft(kind=EventKind.TOPOLOGY_PROPOSED,
                                   payload={"patch_id": patch.patch_id,
                                            "status": "WAITING_BOUNDARY",
                                            "affected_agents": affected})],
                receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                                result={"patch_id": patch.patch_id,
                                        "status": "WAITING_BOUNDARY",
                                        "affected_agents": affected}),
            )
        revision = self.store.save_team_spec(self.session_id, new_spec)
        patch.status = PatchStatus.APPLIED
        self.store.insert_patch(patch, self.session_id)
        self._sync_members(new_spec, spec)
        return Reduction(
            events=[EventDraft(kind=EventKind.TOPOLOGY_APPLIED,
                               payload={"patch_id": patch.patch_id, "revision": revision,
                                        "decided_by": action.actor_id,
                                        "operations": operations_json})],
            receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                            result={"patch_id": patch.patch_id, "revision": revision,
                                    "status": "APPLIED", "affected_agents": affected}),
        )

    def _apply_operations(self, spec: TeamSpec, operations: list[dict]) -> tuple[TeamSpec | None, str | None]:
        data = spec.model_dump(mode="json")
        cfg_tools = set(self.catalog.tools)
        cfg_models = set(self.catalog.models)
        for op in operations:
            if not isinstance(op, dict):
                return None, f"operation must be a mapping, got {type(op).__name__}"
            what = op.get("op")
            match what:
                case "add_agent":
                    agent = op.get("agent") or {}
                    if any(a["id"] == agent.get("id") for a in data["agents"]):
                        return None, f"member {agent.get('id')!r} already exists"
                    if agent.get("model_profile") not in cfg_models:
                        return None, (f"unknown model profile {agent.get('model_profile')!r}; "
                                      "configure it in user config first")
                    for tb in agent.get("tool_bindings") or []:
                        if tb not in cfg_tools and tb not in BUILTIN_TOOL_BINDINGS:
                            return None, f"unknown tool binding {tb!r}"
                    data["agents"].append(agent)
                    for ch in op.get("channels") or []:
                        data["channels"].append(ch)
                    for sp in op.get("shared_spaces") or []:
                        existing = next((s for s in data["shared_spaces"]
                                         if s["id"] == sp.get("id")), None)
                        if existing:
                            for key in ("readers", "writers"):
                                for who in sp.get(key) or []:
                                    if who not in existing[key]:
                                        existing[key].append(who)
                        else:
                            data["shared_spaces"].append(
                                {"id": sp.get("id"), "readers": sp.get("readers") or [],
                                 "writers": sp.get("writers") or []})
                case "remove_agent":
                    agent_id = op.get("agent_id")
                    if agent_id == spec.leader_id:
                        return None, "the Leader cannot be removed"
                    if not any(a["id"] == agent_id for a in data["agents"]):
                        return None, f"unknown member {agent_id!r}"
                    data["agents"] = [a for a in data["agents"] if a["id"] != agent_id]
                    data["channels"] = [c for c in data["channels"]
                                        if c["source"] != agent_id
                                        and agent_id not in c["targets"]]
                    data["observers"] = [o for o in data["observers"]
                                         if o["agent_id"] != agent_id]
                    for s in data["shared_spaces"]:
                        s["readers"] = [x for x in s["readers"] if x != agent_id]
                        s["writers"] = [x for x in s["writers"] if x != agent_id]
                case "update_agent":
                    agent_id = op.get("agent_id")
                    target = next((a for a in data["agents"] if a["id"] == agent_id), None)
                    if target is None:
                        return None, f"unknown member {agent_id!r}"
                    changes = op.get("changes") or {}
                    if "model_profile" in changes and changes["model_profile"] not in cfg_models:
                        return None, f"unknown model profile {changes['model_profile']!r}"
                    for tb in changes.get("tool_bindings") or []:
                        if tb not in cfg_tools and tb not in BUILTIN_TOOL_BINDINGS:
                            return None, f"unknown tool binding {tb!r}"
                    target.update(changes)
                case "add_channel":
                    data["channels"].append(op.get("channel") or {})
                case "remove_channel":
                    src, tgts = op.get("source"), op.get("targets") or []
                    for t in tgts:
                        for c in data["channels"]:
                            if c["source"] == src and t in c["targets"]:
                                c["targets"].remove(t)
                        data["channels"] = [c for c in data["channels"]
                                            if not (c["source"] == src and not c["targets"])]
                case "set_observer":
                    ob = op.get("observer") or {}
                    data["observers"] = [o for o in data["observers"]
                                         if o["agent_id"] != ob.get("agent_id")]
                    if not op.get("remove"):
                        data["observers"].append(ob)
                case "set_space_acl":
                    sid = op.get("space_id")
                    target = next((s for s in data["shared_spaces"] if s["id"] == sid), None)
                    if target is None:
                        return None, f"unknown shared space {sid!r}"
                    for key in ("readers", "writers"):
                        if key in op:
                            target[key] = op[key]
                case _:
                    return None, f"unsupported operation {what!r}"
        try:
            new_spec = TeamSpec.model_validate(data)
        except Exception as e:  # pydantic ValidationError -> readable message
            return None, f"patch produced an invalid team spec: {e}"
        return new_spec, None

    def _affected_agents(self, old: TeamSpec, new: TeamSpec,
                         operations: list[dict]) -> list[str]:
        affected: set[str] = set()

        def add(value: Any) -> None:
            if isinstance(value, str):
                affected.add(value)

        for op in operations:
            match op.get("op"):
                case "remove_agent":
                    add(op.get("agent_id"))
                case "update_agent":
                    add(op.get("agent_id"))
                case "add_channel":
                    ch = op.get("channel") or {}
                    add(ch.get("source"))
                    for t in ch.get("targets") or []:
                        add(t)
                case "remove_channel":
                    add(op.get("source"))
                    for t in op.get("targets") or []:
                        add(t)
                case "set_space_acl":
                    sid = op.get("space_id")
                    old_space = next((s for s in old.shared_spaces if s.id == sid), None)
                    if old_space:
                        affected.update(old_space.readers + old_space.writers)
                    for key in ("readers", "writers"):
                        for who in op.get(key) or []:
                            add(who)
                case "set_observer":
                    ob = op.get("observer") or {}
                    add(ob.get("agent_id"))
                    for s in ob.get("subjects") or []:
                        add(s)
        # keep only members whose identity, config or permissions actually changed
        result: list[str] = []
        for aid in sorted(a for a in affected if a):
            old_a = next((a for a in old.agents if a.id == aid), None)
            new_a = next((a for a in new.agents if a.id == aid), None)
            if old_a is None or new_a is None or old_a != new_a:
                result.append(aid)
            elif self._permissions_changed(old, new, aid):
                result.append(aid)
        return result

    @staticmethod
    def _permissions_changed(old: TeamSpec, new: TeamSpec, agent_id: str) -> bool:
        def perms(spec: TeamSpec) -> tuple:
            sends = sorted(a.id for a in spec.agents if spec.can_send(agent_id, a.id))
            delegates = sorted(a.id for a in spec.agents if spec.can_delegate(agent_id, a.id))
            spaces = sorted(
                (s.id, agent_id in s.readers, agent_id in s.writers)
                for s in spec.shared_spaces
            )
            observers = sorted(
                (o.agent_id, tuple(o.subjects), tuple(sorted(o.event_types)))
                for o in spec.observers
            )
            return sends, delegates, spaces, observers

        return perms(old) != perms(new)

    def _sync_members(self, new: TeamSpec, old: TeamSpec) -> None:
        """Ensure runtime rows exist for new members; bump config revisions on change."""
        for a in new.agents:
            self.store.ensure_agent(self.session_id, a.id)
        for a in new.agents:
            old_a = next((x for x in old.agents if x.id == a.id), None)
            if old_a is None or old_a != a:
                revision = self.store.bump_config_revision(self.session_id, a.id)
                if self.store.agent_status(self.session_id, a.id) is AgentStatus.DRAINING:
                    self.store.set_agent_status(self.session_id, a.id, AgentStatus.IDLE)
                _ = revision
        for a in old.agents:
            if not any(x.id == a.id for x in new.agents):
                self.store.set_agent_status(self.session_id, a.id, AgentStatus.REMOVED)
                self._hand_over_removed_member(a.id, new)

    def _hand_over_removed_member(self, agent_id: str, spec: TeamSpec) -> None:
        """A removed member's unfinished work goes back to the Leader for a new
        decision; results and audit records are kept (plan section 8)."""
        handed = self.store.reassign_tasks(
            agent_id, spec.leader_id, [TaskStatus.PENDING, TaskStatus.BLOCKED])
        for task_id in handed:
            # a handed-over task must be announced to its new assignee
            self.store.del_meta(f"task_ready_announced:{task_id}")
        dropped = self.store.drop_pending_deliveries(
            self.session_id, agent_id, "member removed before delivery")
        action = TeamAction(action_id=f"member-removed:{agent_id}",
                            session_id=self.session_id, actor_id="system",
                            kind=ActionKind.APPLY_TOPOLOGY_PATCH, payload={})
        self._persist_events(action, spec, [EventDraft(
            kind=EventKind.MEMBER_REMOVED,
            payload={"agent_id": agent_id, "hands_over_to": spec.leader_id,
                     "handed_over_tasks": handed, "dropped_deliveries": dropped,
                     "note": "results and work directories are kept for review"},
        )])

    def _cancel_task(self, action: TeamAction, spec: TeamSpec) -> Reduction:
        task = self.store.get_task(action.payload["task_id"])
        if task.status in (TaskStatus.SUCCEEDED, TaskStatus.FAILED, TaskStatus.CANCELLED):
            return Reduction(
                events=[],
                receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                                result={"task_id": task.task_id, "status": task.status}),
            )
        active = self.store.active_run_for_agent(self.session_id, task.assignee)
        if active is None:
            # a turn parked on an approval has no executor: requesting its cancel
            # lets _schedule converge it (approval expires, run -> CANCELLED)
            parked = self._waiting_run(task.assignee)
            if (parked is not None and parked.status is TurnStatus.WAITING_APPROVAL
                    and (parked.task_id == task.task_id or parked.task_id is None)):
                active = parked
        events = [EventDraft(kind=EventKind.TASK_CANCELLED,
                             payload={"task_id": task.task_id, "assignee": task.assignee,
                                      "requester": task.requester,
                                      "status": "CANCEL_REQUESTED"},
                             task_id=task.task_id)]
        if active is not None and (active.task_id == task.task_id or active.task_id is None):
            self.store.set_run_cancel_requested(active.run_id)
            self.store.set_task_cancel_requested(task.task_id)
            status = "CANCEL_REQUESTED"
        else:
            self.store.compare_and_set_task(task.task_id, task.status, TaskStatus.CANCELLED)
            status = "CANCELLED"
        return Reduction(events=events,
                         receipt=Receipt(action_id=action.action_id, ok=True, kind=action.kind,
                                         result={"task_id": task.task_id, "status": status}))

    def _completion_blockers(self, spec: TeamSpec, current_run: str | None = None) -> list[str]:
        blockers: list[str] = []
        live_runs = self.store.runs_for_session(
            self.session_id,
            [TurnStatus.QUEUED, TurnStatus.RUNNING, TurnStatus.WAITING_TASK,
             TurnStatus.WAITING_APPROVAL])
        live_runs = [r for r in live_runs if r.run_id != current_run]
        if live_runs:
            blockers.append("active turns: " + ", ".join(
                f"{r.agent_id}:{r.status}" for r in live_runs))
        unknown = self.store.runs_for_session(self.session_id, [TurnStatus.OUTCOME_UNKNOWN])
        if unknown:
            blockers.append("outcome-unknown operations: " + ", ".join(
                r.run_id for r in unknown))
        un = self.store.tasks_for_session(
            self.session_id,
            [TaskStatus.PENDING, TaskStatus.RUNNING, TaskStatus.BLOCKED])
        if un:
            blockers.append("unfinished tasks: " + ", ".join(
                f"{t.task_id}:{t.status}" for t in un))
        pend = self.store.pending_approvals(self.session_id)
        pending_approvals = [a for a in pend if a is not None]
        if pending_approvals:
            blockers.append("pending approvals: " + ", ".join(
                a.approval_id for a in pending_approvals))
        return blockers

    def expire_run_approvals(self, run_id: str) -> list[EventDraft]:
        """Void a run's PENDING approvals and return the audit events (RT-06).

        A turn that reached a terminal state (or was cancelled) can never use a
        decision on those calls; leaving them pending deadlocks `signal_done`.
        """
        return [EventDraft(kind=EventKind.APPROVAL_DECIDED,
                           payload={"approval_id": a.approval_id, "agent_id": a.agent_id,
                                    "run_id": a.run_id, "status": ApprovalStatus.EXPIRED.value,
                                    "note": "turn ended before a decision"})
                for a in self.store.expire_run_approvals(run_id)]

    # ----------------------------------------------------------------- persist

    def _persist_events(self, action: TeamAction, spec: TeamSpec,
                        drafts: list[EventDraft]) -> None:
        batch_by_agent: dict[str, int] = {}
        for draft in drafts:
            actor = draft.actor_id or action.actor_id
            audience = (draft.audience if draft.audience is not None
                        else views.event_audience(spec, draft.kind, actor, draft.payload,
                                                  draft.targets))
            push = views.event_push(spec, draft.kind, actor, draft.payload, draft.targets)
            if draft.push is not None:
                # explicit recipients are additive; observers still get their subscription
                push = sorted(set(push) | set(draft.push))
            event = TeamEvent(
                event_id=new_id("evt"),
                session_id=self.session_id,
                actor_id=actor,
                task_id=draft.task_id,
                kind=draft.kind,
                payload=draft.payload,
                audience=audience,
                topology_revision=self.store.current_revision(self.session_id),
                causation_id=action.action_id,
            )
            self.store.append_event(event)
            for recipient in push:
                batch = batch_by_agent.get(recipient)
                if batch is None:
                    batch = self.store.next_batch_no(self.session_id, recipient)
                    batch_by_agent[recipient] = batch
                scope = views.observer_scope_for(spec, recipient, draft.kind, actor, draft.payload)
                override = (views.scope_payload(scope, draft.kind, draft.payload)
                            if scope else None)
                self.store.create_delivery(self.session_id, recipient, event.event_id, batch,
                                           payload_override=override)

    # ---------------------------------------------------------------- schedule

    def _schedule(self, spec: TeamSpec) -> None:
        """Create/wake runs for pending deliveries and ready tasks."""
        session = self.session_id
        self._apply_boundary_patches(spec)
        self._block_unrunnable_tasks(spec)
        self._announce_ready_tasks(spec)
        for agent in spec.agents:
            parked = self._waiting_run(agent.id)
            if (parked is not None and parked.status is TurnStatus.WAITING_APPROVAL
                    and parked.cancel_requested and parked.external_turn_id is None):
                # a turn parked on an approval has no executor to finalize it
                # (external backends keep their own live turn and are stopped
                # through the runtime's cancel path instead)
                self._converge_cancelled_waiting_run(parked, spec)
            status = self.store.agent_status(session, agent.id)
            if status in (AgentStatus.REMOVED, AgentStatus.DRAINING):
                continue
            pending = self.store.pending_deliveries(session, agent.id)
            waiting = self._waiting_run(agent.id)
            active = self._active_runs(agent.id)
            if not pending and not active and waiting is None:
                # Liveness: a ready task whose wake notification was consumed
                # mid-turn by an earlier run (and then acked with that run) would
                # otherwise never be dispatched - the member sits IDLE with the
                # task PENDING forever. Dispatch it directly; one turn at a time.
                task = self._next_ready_task(agent.id)
                if task is None:
                    continue
                if not self._turn_budget_ok():
                    self._emit_limit_reached()
                    continue
                self.store.insert_run(TurnRun(
                    run_id=new_id("run"),
                    session_id=session,
                    task_id=task.task_id,
                    goal_id=self._current_goal_id(),
                    agent_id=agent.id,
                    config_revision=self.store.agent_config_revision(session, agent.id),
                    topology_revision=self.store.current_revision(session),
                    status=TurnStatus.QUEUED,
                    input_delivery_ids=[],
                    context_ref=self._context_ref(agent.id),
                ))
                continue
            if not pending:
                continue
            if active:
                run = active[0]
                fresh = [d for d in pending if d["delivery_id"] not in run.input_delivery_ids]
                ids = [d["delivery_id"] for d in fresh]
                if not ids:
                    continue
                self.store.append_run_inputs(run.run_id, ids)
                if run.status is TurnStatus.RUNNING:
                    self.mid_turn_pushes.append((run.run_id, [
                        {"kind": d["event_kind"], "from": d["event_actor"],
                         "task_id": d["event_task_id"],
                         "payload": json.loads(d["payload_json"])}
                        for d in fresh
                    ]))
                continue
            if waiting is not None:
                if waiting.status is TurnStatus.WAITING_APPROVAL:
                    continue
                # ponytail: a waiting turn resumes when ALL waited tasks are
                # terminal, or on user input / cancel. Partial results stay
                # queued; split waits into separate waits if partial wake needed.
                pending_wait = [t for t in waiting.waiting_on
                                if (task := self.store.get_task(t)) is not None
                                and task.status not in (TaskStatus.SUCCEEDED, TaskStatus.FAILED,
                                                        TaskStatus.CANCELLED)]
                seen = set(waiting.input_delivery_ids or [])
                user_input = any(d["event_kind"] == EventKind.USER_MESSAGE
                                 and d["delivery_id"] not in seen for d in pending)
                if pending_wait and not user_input and not waiting.cancel_requested:
                    continue
                if self.store.update_run_status_where(
                        waiting.run_id, TurnStatus.WAITING_TASK, TurnStatus.RUNNING):
                    self.store.append_run_inputs(
                        waiting.run_id, [d["delivery_id"] for d in pending])
                continue
            if not self._turn_budget_ok():
                self._emit_limit_reached()
                continue
            task = self._next_ready_task(agent.id)
            run = TurnRun(
                run_id=new_id("run"),
                session_id=session,
                task_id=task.task_id if task else None,
                goal_id=self._current_goal_id(),
                agent_id=agent.id,
                config_revision=self.store.agent_config_revision(session, agent.id),
                topology_revision=self.store.current_revision(session),
                status=TurnStatus.QUEUED,
                input_delivery_ids=[d["delivery_id"] for d in pending],
                context_ref=self._context_ref(agent.id),
            )
            self.store.insert_run(run)

    def _converge_cancelled_waiting_run(self, run: TurnRun, spec: TeamSpec) -> None:
        """Finalize a run that was cancelled while parked on an approval.

        Nothing is executing, so no runner will ever call `_finalize`: without
        this the run stays WAITING_APPROVAL with a PENDING approval and blocks
        `signal_done` forever (RT-06).
        """
        if not self.store.update_run_status_where(
                run.run_id, TurnStatus.WAITING_APPROVAL, TurnStatus.CANCELLED):
            return
        events = self.expire_run_approvals(run.run_id)
        if run.task_id:
            task = self.store.get_task(run.task_id)
            if task is not None and task.status in (TaskStatus.PENDING, TaskStatus.RUNNING):
                if self.store.compare_and_set_task(task.task_id, task.status,
                                                   TaskStatus.CANCELLED):
                    events.append(EventDraft(
                        kind=EventKind.TASK_CANCELLED,
                        payload={"task_id": task.task_id, "assignee": task.assignee,
                                 "requester": task.requester, "status": "CANCELLED"},
                        task_id=task.task_id))
        self.store.set_agent_status(self.session_id, run.agent_id, AgentStatus.IDLE)
        self.store.ack_run_deliveries(run)
        events.append(EventDraft(
            kind=EventKind.RUN_CANCELLED,
            payload={"run_id": run.run_id, "agent_id": run.agent_id,
                     "status": TurnStatus.CANCELLED.value,
                     "error": "cancelled while waiting for approval"}))
        action = TeamAction(action_id=f"cancel-parked:{run.run_id}",
                            session_id=self.session_id, actor_id="system",
                            kind=ActionKind.CANCEL_RUN, payload={"run_id": run.run_id})
        self._persist_events(action, spec, events)

    def _block_unrunnable_tasks(self, spec: TeamSpec) -> None:
        """Dependencies that failed/cancelled make dependents BLOCKED, never silently stuck."""
        for task in self.store.tasks_for_session(self.session_id, [TaskStatus.PENDING]):
            deps = [self.store.get_task(d) for d in task.dependencies]
            broken = [d for d in deps
                      if d is not None and d.status in (TaskStatus.FAILED, TaskStatus.CANCELLED)]
            if not broken:
                continue
            if self.store.compare_and_set_task(task.task_id, TaskStatus.PENDING,
                                               TaskStatus.BLOCKED):
                draft = EventDraft(
                    kind=EventKind.TASK_BLOCKED,
                    payload={"task_id": task.task_id, "assignee": task.assignee,
                             "requester": task.requester,
                             "reason": "dependency " + ", ".join(
                                 f"{d.task_id}:{d.status}" for d in broken)},
                    task_id=task.task_id,
                )
                action = TeamAction(action_id=f"blocked:{task.task_id}",
                                    session_id=self.session_id, actor_id="system",
                                    kind=ActionKind.CANCEL_TASK, payload={"task_id": task.task_id})
                self._persist_events(action, spec, [draft])

    def _announce_ready_tasks(self, spec: TeamSpec) -> None:
        """One TASK_READY ping per task, once its dependencies are satisfied."""
        for task in sorted(self.store.tasks_for_session(self.session_id, [TaskStatus.PENDING]),
                           key=lambda t: t.created_at):
            deps = [self.store.get_task(d) for d in task.dependencies]
            if not all(d is not None and d.status is TaskStatus.SUCCEEDED for d in deps):
                continue
            if self.store.get_meta(f"task_ready_announced:{task.task_id}"):
                continue
            draft = EventDraft(
                kind=EventKind.TASK_READY,
                payload={"task_id": task.task_id, "assignee": task.assignee,
                         "requester": task.requester,
                         "description": task.description, "acceptance": task.acceptance},
                task_id=task.task_id,
            )
            action = TeamAction(action_id=f"ready:{task.task_id}", session_id=self.session_id,
                                actor_id="system", kind=ActionKind.CANCEL_TASK, payload={})
            self._persist_events(action, spec, [draft])
            self.store.set_meta(f"task_ready_announced:{task.task_id}", "1")

    def _apply_boundary_patches(self, spec: TeamSpec) -> None:
        for patch in self.store.patches_in_status(self.session_id, PatchStatus.WAITING_BOUNDARY):
            if any(self._agent_has_live_run(a) for a in patch.affected_agents):
                continue
            current = self.store.load_team_spec(self.session_id)
            new_spec, error = self._apply_operations(current, patch.operations)
            if error:
                self.store.set_patch_status(patch.patch_id, PatchStatus.FAILED)
                continue
            revision = self.store.save_team_spec(self.session_id, new_spec)
            self.store.set_patch_status(patch.patch_id, PatchStatus.APPLIED)
            self._sync_members(new_spec, current)
            self._persist_patch_event(patch, revision)

    def _persist_patch_event(self, patch: TopologyPatch, revision: int) -> None:
        spec = self.store.load_team_spec(self.session_id)
        draft = EventDraft(kind=EventKind.TOPOLOGY_APPLIED,
                           payload={"patch_id": patch.patch_id, "revision": revision,
                                    "decided_by": patch.decided_by,
                                    "operations": patch.operations})
        action = TeamAction(action_id=f"patch-apply:{patch.patch_id}",
                            session_id=self.session_id, actor_id=patch.decided_by or patch.proposer,
                            kind=ActionKind.APPLY_TOPOLOGY_PATCH, payload={})
        self._persist_events(action, spec, [draft])

    def _emit_limit_reached(self) -> None:
        key = f"limit_notified:{self._current_goal_id()}"
        if self.store.get_meta(key):
            return
        self.store.set_meta(key, "1")
        spec = self.store.load_team_spec(self.session_id)
        draft = EventDraft(kind=EventKind.LIMIT_REACHED,
                           payload={"kind": "max_turns_per_goal",
                                    "limit": spec.limits.max_turns_per_goal,
                                    "goal_id": self._current_goal_id()})
        action = TeamAction(action_id=new_id("sys"), session_id=self.session_id,
                            actor_id="system", kind=ActionKind.PAUSE_SESSION, payload={})
        self._persist_events(action, spec, [draft])

    # ------------------------------------------------------------- small helpers

    def _agent_has_live_run(self, agent_id: str) -> bool:
        return any(r.agent_id == agent_id for r in self.store.runs_for_session(
            self.session_id,
            [TurnStatus.QUEUED, TurnStatus.RUNNING, TurnStatus.WAITING_TASK,
             TurnStatus.WAITING_APPROVAL]))

    def _active_runs(self, agent_id: str) -> list[TurnRun]:
        return [r for r in self.store.runs_for_session(
            self.session_id, [TurnStatus.QUEUED, TurnStatus.RUNNING])
            if r.agent_id == agent_id]

    def _waiting_run(self, agent_id: str) -> TurnRun | None:
        """The member's parked turn (waiting on tasks or on an approval)."""
        for r in self.store.runs_for_session(
                self.session_id, [TurnStatus.WAITING_TASK, TurnStatus.WAITING_APPROVAL]):
            if r.agent_id == agent_id:
                return r
        return None

    def _next_ready_task(self, agent_id: str) -> Task | None:
        for task in sorted(self.store.tasks_for_session(self.session_id, [TaskStatus.PENDING]),
                           key=lambda t: t.created_at):
            if task.assignee != agent_id:
                continue
            deps = [self.store.get_task(d) for d in task.dependencies]
            if all(d is not None and d.status is TaskStatus.SUCCEEDED for d in deps):
                return task
        return None

    def _turn_budget_ok(self) -> bool:
        goal_id = self._current_goal_id()
        if not goal_id:
            return True
        spec = self.store.load_team_spec(self.session_id)
        used = self.store.count_goal_runs(self.session_id, goal_id)
        return used < spec.limits.max_turns_per_goal

    def _task_result(self, task_id: str) -> dict:
        task = self.store.get_task(task_id)
        return {"task_id": task_id, "status": task.status if task else "UNKNOWN",
                "result_refs": task.result_refs if task else []}

    def _context_ref(self, agent_id: str) -> str:
        epoch = self.store.agent_context_epoch(self.session_id, agent_id)
        return f"ctx:{agent_id}:{epoch}"

    def _ensure_goal(self, spec: TeamSpec) -> str:
        session = self.store.get_session(self.session_id)
        goal_id = session["goal_id"] if session else None
        if goal_id and session["goal_state"] in ("active", "done"):
            if session["goal_state"] == "done":
                goal_id = new_id("goal")
                self.store.set_goal_state(self.session_id, goal_id, "active")
            return goal_id
        goal_id = goal_id or new_id("goal")
        self.store.set_goal_state(self.session_id, goal_id, "active")
        return goal_id

    def _current_goal_id(self) -> str | None:
        session = self.store.get_session(self.session_id)
        return session["goal_id"] if session else None

    def _wake_approval_run(self, run_id: str) -> None:
        run = self.store.get_run(run_id)
        if run is not None and run.status is TurnStatus.WAITING_APPROVAL:
            self.store.update_run_status_where(
                run_id, TurnStatus.WAITING_APPROVAL, TurnStatus.RUNNING)
