"""Tool permission modes and approvals (plan section 12.2).

`approved_scope`: pre-authorized operations run; anything else pauses for a
once/session/deny decision bound to the exact operation and its parameters.
`full_auto`: user-opened mode, skips per-call approvals, keeps team ACLs.

Path checking (symlink/traversal) and the bubblewrap scope land in P3; this
module already owns the decision flow so both runners share one gate.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import Any

from .models import ApprovalRequest, ApprovalStatus, PermissionMode, new_id
from .storage import Store


@dataclass
class Decision:
    allow: bool
    scope: dict[str, Any] | None = None
    reason: str = ""


def operation_hash(tool_name: str, args: dict[str, Any]) -> str:
    """Approval is bound to the operation and its parameters (section 12.2)."""
    blob = json.dumps({"tool": tool_name, "args": args}, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(blob.encode()).hexdigest()[:32]


class PermissionPolicy:
    """Evaluates one tool call. Overridden/configured per deployment."""

    def __init__(self, mode: PermissionMode = PermissionMode.APPROVED_SCOPE,
                 pre_authorized: set[str] | None = None,
                 require_approval: set[str] | None = None,
                 write_paths: list[Any] | None = None):
        self.mode = mode
        #: tools that may run without asking inside the approved scope
        self.pre_authorized = pre_authorized if pre_authorized is not None else {"files", "shell"}
        #: tools that always ask in approved_scope (e.g. network egress)
        self.require_approval = require_approval or set()
        #: directories the user pre-authorized for file writes (real paths)
        self.write_paths = [str(p) for p in (write_paths or [])]

    def evaluate(self, tool_name: str, args: dict[str, Any]) -> Decision:
        """Decide one call. Unknown tools ask; approved scope is allow-by-default
        only for tools the runtime actually bound to the member."""
        if self.mode is PermissionMode.FULL_AUTO:
            return Decision(allow=True)
        if tool_name == "shell" and args.get("network"):
            return Decision(allow=False,
                            scope={"tool": tool_name, "args": args,
                                   "reason": "shell network access is off by default"},
                            reason="shell network access requires approval")
        if tool_name in self.require_approval:
            return Decision(allow=False,
                            scope={"tool": tool_name, "args": args,
                                   "reason": f"{tool_name} needs approval"},
                            reason="outside pre-authorized scope")
        if tool_name in self.pre_authorized or self._bound_tool(tool_name):
            return Decision(allow=True)
        if tool_name.startswith(("mcp_", "web_")):
            return Decision(allow=True)
        return Decision(allow=False,
                        scope={"tool": tool_name, "args": args,
                               "reason": f"{tool_name} is not pre-authorized"},
                        reason="tool not in approved scope")

    @staticmethod
    def _bound_tool(tool_name: str) -> bool:
        """Runtime-bound execution tools: file tools stay inside the sandboxed
        backend and MCP/web tools exist only when the user configured them."""
        file_tools = {"ls", "read_file", "write_file", "edit_file", "delete",
                      "glob", "grep", "read_artifact"}
        return tool_name in file_tools


class ApprovalGate:
    """Creates approval requests and resolves them once/session/deny."""

    def __init__(self, store: Store, session_id: str, policy: PermissionPolicy):
        self.store = store
        self.session_id = session_id
        self.policy = policy
        self.policy_revision = 1

    def set_mode(self, mode: PermissionMode) -> None:
        self.policy.mode = mode
        self.policy_revision += 1

    def check(self, agent_id: str, run_id: str, tool_name: str, args: dict[str, Any],
              tool_call_id: str) -> tuple[Decision, ApprovalRequest | None]:
        """Returns (decision, approval_request). An open request means: pause."""
        decision = self.policy.evaluate(tool_name, args)
        if decision.allow:
            return decision, None
        op_hash = operation_hash(tool_name, args)
        if self.store.find_session_approval(self.session_id, op_hash) is not None:
            return Decision(allow=True), None
        existing = self.store.approval_for_call(run_id, tool_call_id, op_hash)
        if existing is not None:
            match existing.status:
                case ApprovalStatus.PENDING:
                    return decision, existing
                case ApprovalStatus.DENIED:
                    return Decision(allow=False, reason="denied by the user"), existing
                case ApprovalStatus.APPROVED_ONCE | ApprovalStatus.APPROVED_SESSION:
                    if existing.policy_revision == self.policy_revision:
                        return Decision(allow=True), existing
        req = ApprovalRequest(
            approval_id=new_id("appr"),
            session_id=self.session_id,
            agent_id=agent_id,
            run_id=run_id,
            tool_call_id=tool_call_id,
            operation_hash=op_hash,
            requested_scope=decision.scope or {},
            policy_revision=self.policy_revision,
        )
        self.store.insert_approval(req)
        return decision, req

    def recheck(self, approval_id: str, tool_name: str, args: dict[str, Any]) -> bool:
        """On resume: re-verify parameters and the recorded decision (section 12.2)."""
        req = self.store.get_approval(approval_id)
        if req is None:
            return False
        if req.operation_hash != operation_hash(tool_name, args):
            return False
        if req.status is ApprovalStatus.APPROVED_SESSION:
            return True
        return (req.status is ApprovalStatus.APPROVED_ONCE
                and req.policy_revision == self.policy_revision)

    def consume_once(self, approval_id: str) -> None:
        self.store.expire_approval(approval_id)

    def register_external(self, *, agent_id: str, run_id: str, tool_call_id: str,
                          scope: dict[str, Any]) -> ApprovalRequest:
        """Record an approval request raised by an external backend (Codex), where
        the operation's own service defines the scope and our policy is not the
        gate; the user decision still flows through the same approval queue."""
        op_hash = operation_hash(scope.get("kind", "external"), scope.get("request", {}))
        req = ApprovalRequest(
            approval_id=new_id("appr"), session_id=self.session_id, agent_id=agent_id,
            run_id=run_id, tool_call_id=tool_call_id, operation_hash=op_hash,
            requested_scope=scope, policy_revision=self.policy_revision,
        )
        self.store.insert_approval(req)
        return req
